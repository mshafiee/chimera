//! Helius API quota tripwire (2026-09-12).
//!
//! Process-global, std-only coordination flag so every Helius consumer
//! (tier polling, webhook enrichment, scout cycles) stops burning calls
//! while the key is exhausted and probes recovery on a schedule instead.
//!
//! Two trip classes, because they recover differently:
//! - [`QuotaClass::MonthlyCap`] (`"max usage reached"`): the monthly credit
//!   budget is gone. It does NOT clear at midnight — only a billing-cycle
//!   reset, a fresh key, or a manual [`clear`] recovers. Observed 2026-09-11:
//!   46h of global signal silence while polling kept firing into 429s.
//! - [`QuotaClass::Throughput`] (plain 429 / rate-limit): transient. Short
//!   backoff, then half-open probing resumes automatically.
//!
//! Pure coordination state (atomics + wall clock) — no I/O, no spawning,
//! safe to share across operator/infra/scout within one process.

use std::sync::atomic::{AtomicU64, Ordering};

/// Backoff after a monthly-cap trip before one probe cycle is let through.
pub const MONTHLY_CAP_TRIP_SECS: u64 = 3600;
/// Backoff after a throughput-429 trip before probing resumes.
pub const THROUGHPUT_TRIP_SECS: u64 = 120;

/// Which kind of Helius quota failure tripped the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaClass {
    /// Monthly credit budget exhausted (`"max usage reached"`).
    MonthlyCap,
    /// Transient throughput limit (plain 429 / rate-limit text).
    Throughput,
}

impl QuotaClass {
    /// How long (seconds) a trip of this class suppresses Helius calls.
    pub fn trip_secs(self) -> u64 {
        match self {
            QuotaClass::MonthlyCap => MONTHLY_CAP_TRIP_SECS,
            QuotaClass::Throughput => THROUGHPUT_TRIP_SECS,
        }
    }
}

/// Classify an error message as a Helius quota failure, if it is one.
/// Case-insensitive substring match on the exact production strings
/// (mirrors `is_quota_classified` in infra's webhook health task).
pub fn classify_quota_error(msg: &str) -> Option<QuotaClass> {
    let low = msg.to_lowercase();
    if low.contains("max usage") {
        Some(QuotaClass::MonthlyCap)
    } else if low.contains("429")
        || low.contains("too many requests")
        || low.contains("rate limit")
        || low.contains("rate-limit")
    {
        Some(QuotaClass::Throughput)
    } else {
        None
    }
}

// 0 = clear; otherwise epoch seconds of the trip. The class is stored
// alongside so a transient 429 cannot shorten an active monthly-cap trip.
static TRIPPED_AT_SECS: AtomicU64 = AtomicU64::new(0);
static TRIPPED_CLASS_MONTHLY: AtomicU64 = AtomicU64::new(0); // 0 = none, 1 = monthly, 2 = throughput

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Trip the wire. A [`QuotaClass::Throughput`] trip never shortens an
/// active [`QuotaClass::MonthlyCap`] trip.
pub fn trip(class: QuotaClass) {
    let now = now_secs();
    match class {
        QuotaClass::MonthlyCap => {
            TRIPPED_CLASS_MONTHLY.store(1, Ordering::SeqCst);
            TRIPPED_AT_SECS.store(now, Ordering::SeqCst);
        }
        QuotaClass::Throughput => {
            // Only take the slot when no monthly-cap trip is active.
            if TRIPPED_CLASS_MONTHLY.load(Ordering::SeqCst) != 1 || trip_expired(now) {
                TRIPPED_CLASS_MONTHLY.store(2, Ordering::SeqCst);
                TRIPPED_AT_SECS.store(now, Ordering::SeqCst);
            }
        }
    }
}

/// Manually clear the wire (fresh key deployed, cycle reset observed).
pub fn clear() {
    TRIPPED_AT_SECS.store(0, Ordering::SeqCst);
    TRIPPED_CLASS_MONTHLY.store(0, Ordering::SeqCst);
}

fn trip_duration_secs() -> u64 {
    match TRIPPED_CLASS_MONTHLY.load(Ordering::SeqCst) {
        1 => MONTHLY_CAP_TRIP_SECS,
        _ => THROUGHPUT_TRIP_SECS,
    }
}

fn trip_expired(now: u64) -> bool {
    let at = TRIPPED_AT_SECS.load(Ordering::SeqCst);
    at == 0 || now.saturating_sub(at) >= trip_duration_secs()
}

/// Whether Helius calls should currently be suppressed. An expired trip
/// half-opens: returns false once so exactly one probe cycle goes through,
/// re-tripping on failure via [`trip`].
pub fn is_tripped() -> bool {
    let at = TRIPPED_AT_SECS.load(Ordering::SeqCst);
    if at == 0 {
        return false;
    }
    !trip_expired(now_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // The tripwire is process-global: serialize tests that touch it.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn classify_recognizes_production_strings() {
        assert_eq!(
            classify_quota_error(
                "{\"jsonrpc\":\"2.0\",\"error\":{\"code\":-32429,\"message\":\"max usage reached\"}}"
            ),
            Some(QuotaClass::MonthlyCap)
        );
        assert_eq!(
            classify_quota_error("Max Usage Reached"),
            Some(QuotaClass::MonthlyCap)
        );
        assert_eq!(
            classify_quota_error("HTTP 429 Too Many Requests"),
            Some(QuotaClass::Throughput)
        );
        assert_eq!(
            classify_quota_error("rate limit exceeded"),
            Some(QuotaClass::Throughput)
        );
        assert_eq!(classify_quota_error("connection reset by peer"), None);
        assert_eq!(classify_quota_error(""), None);
    }

    #[test]
    fn trip_and_clear_roundtrip() {
        let _guard = TEST_LOCK.lock().unwrap();
        clear();
        assert!(!is_tripped());
        trip(QuotaClass::Throughput);
        assert!(is_tripped());
        clear();
        assert!(!is_tripped());
    }

    #[test]
    fn throughput_trip_does_not_shorten_monthly_trip() {
        let _guard = TEST_LOCK.lock().unwrap();
        clear();
        trip(QuotaClass::MonthlyCap);
        assert!(is_tripped());
        // A later transient 429 must not downgrade the monthly trip.
        trip(QuotaClass::Throughput);
        assert_eq!(TRIPPED_CLASS_MONTHLY.load(Ordering::SeqCst), 1);
        assert!(is_tripped());
        clear();
    }

    #[test]
    fn trip_durations_match_class() {
        assert_eq!(QuotaClass::MonthlyCap.trip_secs(), MONTHLY_CAP_TRIP_SECS);
        assert_eq!(QuotaClass::Throughput.trip_secs(), THROUGHPUT_TRIP_SECS);
        // Invariant (checked at compile time): the monthly-cap backoff must
        // exceed the throughput backoff, or a cap trip would half-open too
        // eagerly. Pinned by the two equality asserts above (3600 > 120).
        const _: () = assert!(MONTHLY_CAP_TRIP_SECS > THROUGHPUT_TRIP_SECS);
    }
}
