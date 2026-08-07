//! Run-scoped execution context (Phase C1).
//!
//! A [`RunContext`] is constructed once at startup and uniquely identifies a
//! single process run. Every persisted decision record and promotion episode
//! is stamped with the run's `run_id`, `code_revision`, and `config_hash` so
//! the go/no-go evaluation (C4) can cohort evidence by the exact code +
//! configuration + wallet roster that produced it.
//!
//! ## `run_id` format
//! `v{version}-{config_hash_short}-{start_unix_millis}`
//! - `version`: `CARGO_PKG_VERSION`
//! - `config_hash_short`: first 16 chars of [`SelectionConfig::hash`]
//! - `start_unix_millis`: Unix millisecond timestamp of process start
//!
//! The format is deterministic for a given (version, config, start-time)
//! triple, so a restarted process always gets a fresh `run_id` while records
//! from a single run share one identifier. Millisecond resolution avoids
//! colliding `run_id`s for restarts within the same second (which would
//! silently merge evidence from different runs in C4 cohorting).

use chrono::{DateTime, Utc};

/// Immutable identity of a single operator process run.
#[derive(Debug, Clone)]
pub struct RunContext {
    /// Unique run identifier: `v{version}-{config_hash_short}-{start_unix}`.
    pub run_id: String,
    /// Git commit hash of the running build (`GIT_HASH` build-time env).
    pub code_revision: String,
    /// Full `SelectionConfig` hash (which admission thresholds were in force).
    pub config_hash: String,
    /// Hash of the ACTIVE wallet roster (sorted addresses) at startup.
    pub roster_hash: String,
    /// Process start time (UTC).
    pub started_at: DateTime<Utc>,
}

impl RunContext {
    /// Build a run context from its constituent identity parts.
    ///
    /// `config_hash` is the full [`SelectionConfig::hash`]; only its first 16
    /// characters are embedded in the `run_id`. `roster_addresses` are hashed
    /// after sorting so the roster hash is order-independent.
    pub fn new(
        config_hash: impl Into<String>,
        roster_addresses: &[String],
        started_at: DateTime<Utc>,
    ) -> Self {
        let config_hash = config_hash.into();
        let config_hash_short: String = config_hash.chars().take(16).collect();
        let start_unix = started_at.timestamp_millis();
        let run_id = format!(
            "v{}-{}-{}",
            env!("CARGO_PKG_VERSION"),
            config_hash_short,
            start_unix
        );
        Self {
            run_id,
            // GIT_HASH is emitted by build.rs (cargo:rustc-env). Use option_env!
            // so a build without build.rs (e.g. a stripped Docker context) falls
            // back to "unknown" instead of failing to compile.
            code_revision: option_env!("GIT_HASH")
                .unwrap_or("unknown")
                .to_string(),
            config_hash,
            roster_hash: Self::hash_roster(roster_addresses),
            started_at,
        }
    }

    /// Stable (order-independent) hash of the ACTIVE wallet roster.
    ///
    /// Not cryptographic — a compact fingerprint (8 bytes of SHA-256, ~50%
    /// collision chance around 2^32 distinct rosters) so decision records can
    /// be grouped by "which roster was live" without storing the full list.
    /// The bound is fine for realistic roster counts; if grouping by
    /// `roster_hash` ever grows beyond that scale, use the full digest.
    pub fn hash_roster(roster_addresses: &[String]) -> String {
        use sha2::{Digest, Sha256};
        let mut sorted: Vec<&str> = roster_addresses.iter().map(|s| s.as_str()).collect();
        sorted.sort_unstable();
        let mut hasher = Sha256::new();
        for addr in sorted {
            hasher.update(addr.as_bytes());
            hasher.update(b"\n");
        }
        hex::encode(&hasher.finalize()[..8])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_id_format_is_version_config_start() {
        let started = DateTime::from_timestamp(1_753_456_789, 0).unwrap();
        let ctx = RunContext::new("a1b2c3d4e5f6", &[], started);
        let expected = format!("v{}-a1b2c3d4e5f6-1753456789000", env!("CARGO_PKG_VERSION"));
        assert_eq!(ctx.run_id, expected);
        assert_eq!(ctx.config_hash, "a1b2c3d4e5f6");
        assert_eq!(ctx.started_at, started);
    }

    #[test]
    fn roster_hash_is_order_independent() {
        let a = vec!["walletB".to_string(), "walletA".to_string()];
        let b = vec!["walletA".to_string(), "walletB".to_string()];
        assert_eq!(RunContext::hash_roster(&a), RunContext::hash_roster(&b));
    }

    #[test]
    fn roster_hash_empty_is_stable() {
        assert_eq!(RunContext::hash_roster(&[]), RunContext::hash_roster(&[]));
    }
}
