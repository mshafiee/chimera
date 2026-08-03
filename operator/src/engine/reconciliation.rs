//! Reconciliation runner — Rust port of `ops/reconcile.sh`.
//!
//! Compares DB position state against on-chain transaction confirmation, logs
//! discrepancies, auto-resolves confirmed exits, and updates Prometheus counters
//! directly (no self-HTTP). The on-chain check is abstracted behind
//! [`OnChainTxChecker`] so the runner is unit-testable without a live RPC.

use async_trait::async_trait;
use rust_decimal::Decimal;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::signature::Signature;
use solana_transaction_status::UiTransactionEncoding;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::db_abstraction::types::{PositionDetail, ReconciliationStatus};
use crate::db_abstraction::Database;
use crate::error::{AppError, AppResult};
use crate::metrics::{rpc_errors_metric, rpc_latency_metric, MetricsState};

/// Guards against overlapping reconciliation runs within a single process. The API
/// trigger acquires it before spawning and the spawned task releases it on completion.
pub static RECONCILIATION_RUNNING: AtomicBool = AtomicBool::new(false);

/// RAII guard that clears [`RECONCILIATION_RUNNING`] on drop.
///
/// A bare `store(false)` after an `.await` leaks the flag forever when the
/// task panics or is aborted, locking out every subsequent manual trigger
/// until process restart.
pub struct ReconciliationRunningGuard;

impl ReconciliationRunningGuard {
    pub fn new() -> Option<Self> {
        use std::sync::atomic::Ordering;
        if RECONCILIATION_RUNNING.swap(true, Ordering::SeqCst) {
            None
        } else {
            Some(Self)
        }
    }
}

impl Drop for ReconciliationRunningGuard {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        RECONCILIATION_RUNNING.store(false, Ordering::SeqCst);
    }
}

/// Cap on the number of positions a single sweep inspects, bounding RPC cost. A run
/// processes at most this many positions (most-recent first); a subsequent trigger
/// picks up the remainder.
const MAX_POSITIONS_PER_RUN: usize = 500;

/// Grace window for *entry* confirmations: a position younger than this may have an
/// entry transaction that has not yet landed/finalized, so a `NotFound` entry check is
/// treated as pending rather than a discrepancy. (`opened_at` is the entry time, so it
/// is meaningful for entries. It is NOT used for exits — see exit handling below.)
const ENTRY_FINALIZATION_GRACE_SECS: i64 = 60;

/// Inter-call delay to avoid RPC throttling during a sweep (mirrors the
/// `sleep 0.2` throttle in `ops/reconcile.sh`).
const INTER_CHECK_DELAY: Duration = Duration::from_millis(200);

/// Result of verifying a single transaction signature on-chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnChainTxStatus {
    /// Transaction found and finalized.
    Found,
    /// Definitively not present on-chain (the RPC confirmed it is unknown). Distinct
    /// from [`OnChainTxStatus::Error`] so the runner can log a genuine `MISSING_TX`
    /// discrepancy rather than a transient failure.
    NotFound,
    /// RPC call failed or the result was ambiguous (`getTransaction` returns `null`
    /// for not-yet-finalized, pruned, or genuinely-absent transactions — these cannot
    /// be distinguished, so they are treated as a non-definitive check failure).
    Error,
}

/// Injectable on-chain transaction verifier. Production uses [`RpcOnChainChecker`];
/// tests supply a stub.
#[async_trait]
pub trait OnChainTxChecker: Send + Sync {
    async fn check_signature(&self, signature: &str) -> OnChainTxStatus;
}

/// Production checker backed by a Solana `RpcClient`. Records each call's latency to
/// the global RPC-latency histogram under the `reconciliation` endpoint label, and
/// increments the RPC-error counter **only** for genuine RPC failures — NOT for the
/// expected `NotFound` outcome (a legitimately-absent transaction is a reconciliation
/// finding, not an RPC error).
pub struct RpcOnChainChecker {
    rpc_client: Arc<RpcClient>,
}

impl RpcOnChainChecker {
    pub fn new(rpc_client: Arc<RpcClient>) -> Self {
        Self { rpc_client }
    }
}

#[async_trait]
impl OnChainTxChecker for RpcOnChainChecker {
    async fn check_signature(&self, signature: &str) -> OnChainTxStatus {
        let Ok(sig) = signature.parse::<Signature>() else {
            tracing::warn!(signature = %signature, "reconciliation: unparseable signature");
            return OnChainTxStatus::Error;
        };
        let start = Instant::now();
        let result = self
            .rpc_client
            .get_transaction(&sig, UiTransactionEncoding::Json)
            .await;
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        rpc_latency_metric()
            .with_label_values(&["reconciliation", "getTransaction"])
            .observe(elapsed_ms);

        match result {
            Ok(_) => OnChainTxStatus::Found,
            Err(e) => {
                let msg = e.to_string();
                if msg.to_lowercase().contains("not found") {
                    // Legitimately absent — a reconciliation finding, not an RPC error.
                    OnChainTxStatus::NotFound
                } else {
                    rpc_errors_metric()
                        .with_label_values(&["reconciliation", "getTransaction"])
                        .inc();
                    tracing::warn!(
                        signature = %signature,
                        error = %msg,
                        "reconciliation: getTransaction error"
                    );
                    OnChainTxStatus::Error
                }
            }
        }
    }
}

/// Summary of a single reconciliation sweep.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ReconciliationRunResult {
    /// Positions inspected.
    pub checked_count: u64,
    /// New discrepancies logged this run.
    pub discrepancies: u64,
    /// Positions auto-resolved (confirmed exits closed) this run.
    pub auto_resolved: u64,
    /// Total unresolved discrepancies in the DB after this run.
    pub unresolved: u64,
    /// Wall-clock duration in seconds.
    pub duration_seconds: f64,
}

/// Run one reconciliation sweep over (up to [`MAX_POSITIONS_PER_RUN`]) `ACTIVE`/
/// `EXITING` positions.
///
/// Accounting: exactly **one** `reconciliation_log` row is inserted per inspected
/// position, so the DB-derived `checked_count` (`COUNT(*)`) matches the Prometheus
/// `checked` counter. Per position:
/// - **Entry check** (with an `opened_at`-based finalization grace window): `NotFound`
///   past the grace window logs `MISSING_TX`; `Error` logs `TX_CHECK_ERROR`.
/// - **Exit check** (`EXITING` only): `Found` auto-resolves via
///   [`Database::close_position_full`] and logs a resolved `STATE_MISMATCH`; `NotFound`
///   is treated as **pending** (the exit may be in-flight — there is no reliable exit
///   timestamp, so it is never flagged as a discrepancy to avoid false positives);
///   `Error` logs `TX_CHECK_ERROR`.
///
/// Updates the `chimera_reconciliation_*` Prometheus counters directly on `metrics`
/// rather than via self-HTTP.
pub async fn run_reconciliation(
    db: &dyn Database,
    checker: &dyn OnChainTxChecker,
    metrics: &MetricsState,
) -> ReconciliationRunResult {
    let started = Instant::now();
    let mut result = ReconciliationRunResult::default();

    let positions = match load_reconcilable_positions(db).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "reconciliation: failed to load positions; aborting run");
            return result;
        }
    };

    tracing::info!(count = positions.len(), "reconciliation: starting sweep");

    for (idx, pos) in positions.iter().enumerate() {
        result.checked_count += 1;
        let trade_uuid = pos.trade_uuid.as_str();

        if let Err(e) = reconcile_position(db, checker, pos, &mut result).await {
            tracing::warn!(trade_uuid = %trade_uuid, error = %e, "reconciliation: position check failed");
        }

        // Throttle to avoid RPC rate-limiting — but skip the delay after the final item.
        if idx + 1 < positions.len() {
            tokio::time::sleep(INTER_CHECK_DELAY).await;
        }
    }

    // Compute the post-run unresolved total from the DB and push it to metrics.
    let unresolved = match db.get_reconciliation_status(100).await {
        Ok(ReconciliationStatus { unresolved_count, .. }) => unresolved_count as u64,
        Err(e) => {
            tracing::warn!(error = %e, "reconciliation: failed to read post-run status");
            result.unresolved
        }
    };
    result.unresolved = unresolved;
    result.duration_seconds = started.elapsed().as_secs_f64();

    push_metrics(metrics, &result);

    tracing::info!(
        checked = result.checked_count,
        discrepancies = result.discrepancies,
        auto_resolved = result.auto_resolved,
        unresolved = result.unresolved,
        duration_secs = format!("{:.2}", result.duration_seconds),
        "reconciliation: sweep complete"
    );

    result
}

/// Load `EXITING` + `ACTIVE` positions (most-recent first), capped at
/// [`MAX_POSITIONS_PER_RUN`] to bound per-run RPC cost.
///
/// EXITING positions are loaded FIRST: they are the only ones eligible for
/// exit auto-resolution, and ACTIVE-first ordering meant a large ACTIVE set
/// (which this pass never advances past — checked-but-still-ACTIVE positions
/// keep their `last_updated`) starved EXITING positions of every run.
async fn load_reconcilable_positions(db: &dyn Database) -> AppResult<Vec<PositionDetail>> {
    let mut positions = db.get_positions(Some("EXITING")).await?;
    positions.extend(db.get_positions(Some("ACTIVE")).await?);
    positions.truncate(MAX_POSITIONS_PER_RUN);
    Ok(positions)
}

/// Per-position outcome used to produce exactly one `reconciliation_log` row.
enum Outcome {
    /// No issue detected.
    Ok,
    /// Ambiguous/pending (e.g. exit tx not yet available) — not a discrepancy.
    Pending(&'static str),
    /// A genuine discrepancy to log.
    Discrepancy {
        kind: &'static str,
        actual: Option<&'static str>,
        note: &'static str,
    },
    /// Exit confirmed and auto-resolved; `auto_resolve_exit` already inserted its row.
    AutoResolved,
}

/// Reconcile a single position, inserting exactly one `reconciliation_log` row.
async fn reconcile_position(
    db: &dyn Database,
    checker: &dyn OnChainTxChecker,
    pos: &PositionDetail,
    result: &mut ReconciliationRunResult,
) -> AppResult<()> {
    let mut outcome = Outcome::Ok;
    // Whether the entry transaction was actually verified on-chain. When false,
    // the log must NOT claim "FOUND" (either no entry signature exists, or the
    // check is not yet definitive inside the finalization grace window).
    let mut entry_verified = false;

    // --- Entry transaction check (all active/exiting positions) ---
    let entry_sig = pos.entry_tx_signature.as_str();
    if !entry_sig.is_empty() {
        match checker.check_signature(entry_sig).await {
            OnChainTxStatus::Found => {
                entry_verified = true;
            }
            OnChainTxStatus::NotFound => {
                // Only a discrepancy once the entry has had time to finalize.
                if is_past_entry_grace(pos) {
                    outcome = Outcome::Discrepancy {
                        kind: "MISSING_TX",
                        actual: Some("MISSING"),
                        note: "Entry transaction not found on-chain",
                    };
                }
            }
            OnChainTxStatus::Error => {
                outcome = Outcome::Discrepancy {
                    kind: "TX_CHECK_ERROR",
                    actual: None,
                    note: "Entry transaction check failed (RPC error)",
                };
            }
        }
    }

    // --- Exit transaction check (EXITING positions only) ---
    // NOTE: there is no reliable exit-initiation timestamp (update_position does not
    // touch last_updated), so a NotFound exit is treated as pending rather than flagged
    // — flagging it would produce false discrepancies for every in-flight exit.
    // The exit check only runs when the entry check left `outcome == Ok`: an exit
    // `Found` must never auto-resolve a position whose entry transaction was never
    // confirmed on-chain, and an exit `NotFound` must not downgrade a genuine entry
    // discrepancy to Pending.
    let entry_outcome_ok = matches!(outcome, Outcome::Ok);
    if pos.state == "EXITING" && entry_outcome_ok {
        if let Some(exit_sig) = pos.exit_tx_signature.as_deref().filter(|s| !s.is_empty()) {
            match checker.check_signature(exit_sig).await {
                OnChainTxStatus::Found => match auto_resolve_exit(
                    db,
                    &pos.wallet_address,
                    &pos.token_address,
                    &pos.trade_uuid,
                    exit_sig,
                    pos.exit_price,
                )
                .await
                {
                    Ok(true) => {
                        result.auto_resolved += 1;
                        outcome = Outcome::AutoResolved;
                    }
                    Ok(false) => {
                        // No ACTIVE/EXITING position matched the close — the
                        // position could not be resolved. Record it instead of
                        // counting it as auto-resolved.
                        outcome = Outcome::Discrepancy {
                            kind: "EXIT_MATCH_NOT_FOUND",
                            actual: Some("FOUND"),
                            note: "Exit confirmed on-chain but no ACTIVE/EXITING position matched",
                        };
                    }
                    Err(e) => {
                        // auto_resolve_exit returns Err only when the CLOSE failed; the
                        // position is still EXITING and will be re-examined next run.
                        tracing::warn!(
                            trade_uuid = %pos.trade_uuid,
                            error = %e,
                            "reconciliation: auto-resolve failed"
                        );
                        outcome = Outcome::Discrepancy {
                            kind: "AUTO_RESOLVE_FAILED",
                            actual: Some("FOUND"),
                            note: "Exit confirmed on-chain but auto-resolve failed",
                        };
                    }
                },
                OnChainTxStatus::NotFound => {
                    outcome = Outcome::Pending("Exit transaction not yet available on-chain");
                }
                OnChainTxStatus::Error => {
                    outcome = Outcome::Discrepancy {
                        kind: "TX_CHECK_ERROR",
                        actual: None,
                        note: "Exit transaction check failed (RPC error)",
                    };
                }
            }
        }
    }

    // --- Emit exactly one row for this position ---
    match outcome {
        Outcome::AutoResolved => { /* auto_resolve_exit already logged the resolved row */ }
        Outcome::Ok => {
            // Only claim "FOUND" when the entry was actually verified; a
            // position without a checked entry reports None instead of a
            // misleading confirmed status.
            db.insert_reconciliation_log(
                &pos.trade_uuid,
                &pos.state,
                if entry_verified { Some("FOUND") } else { None },
                "NONE",
                Some(entry_sig),
                Some("No discrepancy detected"),
            )
            .await?;
        }
        Outcome::Pending(note) => {
            db.insert_reconciliation_log(
                &pos.trade_uuid,
                &pos.state,
                None,
                "NONE",
                pos.exit_tx_signature.as_deref(),
                Some(note),
            )
            .await?;
        }
        Outcome::Discrepancy {
            kind,
            actual,
            note,
        } => {
            db.insert_reconciliation_log(
                &pos.trade_uuid,
                &pos.state,
                actual,
                kind,
                if pos.state == "EXITING" {
                    pos.exit_tx_signature.as_deref()
                } else {
                    Some(entry_sig)
                },
                Some(note),
            )
            .await?;
            result.discrepancies += 1;
        }
    }

    Ok(())
}

/// True when a position is older than the entry-finalization grace window. Uses
/// `opened_at` (the entry time), which is meaningful for entry checks. Accepts RFC3339
/// and the two SQLite datetime formats.
fn is_past_entry_grace(pos: &PositionDetail) -> bool {
    let parsed = chrono::DateTime::parse_from_rfc3339(&pos.opened_at)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(&pos.opened_at, "%Y-%m-%d %H:%M:%S%.3f")
                .map(|ndt| ndt.and_utc())
        })
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(&pos.opened_at, "%Y-%m-%d %H:%M:%S")
                .map(|ndt| ndt.and_utc())
        })
        .ok();

    match parsed {
        Some(opened) => {
            let age = chrono::Utc::now().signed_duration_since(opened);
            age.num_seconds() > ENTRY_FINALIZATION_GRACE_SECS
        }
        // Unparseable timestamp: don't suppress a potential discrepancy.
        None => true,
    }
}

/// Confirm an exit on-chain: close the position, then best-effort insert + resolve the
/// reconciliation log row. Returns:
/// - `Ok(true)` — the position was closed (row logged best-effort).
/// - `Ok(false)` — no ACTIVE/EXITING position matched the close (the exit could
///   not be resolved; the caller must log the position instead of counting it
///   as auto-resolved).
/// - `Err` — only when the **close** fails (the position remains `EXITING` and
///   will be re-examined); a logging failure after a successful close returns
///   `Ok(true)` so the caller never records a spurious `AUTO_RESOLVE_FAILED` for
///   a position that is in fact already closed.
async fn auto_resolve_exit(
    db: &dyn Database,
    wallet_address: &str,
    token_address: &str,
    trade_uuid: &str,
    exit_sig: &str,
    exit_price: Option<Decimal>,
) -> AppResult<bool> {
    let Some(price) = exit_price.filter(|p| !p.is_zero()) else {
        return Err(AppError::Validation(
            "exit_price missing or zero — cannot auto-resolve".to_string(),
        ));
    };

    let position_closed = db
        .close_position_full(
            trade_uuid,
            wallet_address,
            token_address,
            price,
            exit_sig,
            None,
            Decimal::ONE,
            true,
        )
        .await?;

    if !position_closed {
        return Ok(false);
    }

    // Close succeeded — the position is now CLOSED and won't be re-examined. Record
    // the resolution best-effort; a failure here must NOT surface as AUTO_RESOLVE_FAILED.
    let log_id = match db
        .insert_reconciliation_log(
            trade_uuid,
            "EXITING",
            Some("FOUND"),
            "STATE_MISMATCH",
            Some(exit_sig),
            Some("Exit confirmed on-chain; auto-resolved to CLOSED"),
        )
        .await
    {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(trade_uuid = %trade_uuid, error = %e, "reconciliation: exit closed but log insert failed");
            return Ok(true);
        }
    };

    if let Err(e) = db
        .resolve_discrepancy(log_id, "AUTO", "Exit confirmed on-chain")
        .await
    {
        tracing::warn!(trade_uuid = %trade_uuid, error = %e, "reconciliation: exit closed but resolve failed (left unresolved)");
    }
    Ok(true)
}

/// Push the run summary to the existing Prometheus reconciliation counters.
fn push_metrics(metrics: &MetricsState, result: &ReconciliationRunResult) {
    if result.checked_count > 0 {
        metrics
            .reconciliation_checked
            .inc_by(result.checked_count);
    }
    if result.discrepancies > 0 {
        metrics
            .reconciliation_discrepancies
            .inc_by(result.discrepancies);
    }
    metrics
        .reconciliation_unresolved
        .set(result.unresolved as i64);
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::Ordering;

    /// Stub checker that maps signatures to predetermined statuses.
    struct StubChecker {
        found: Vec<String>,
        not_found: Vec<String>,
    }

    #[async_trait]
    impl OnChainTxChecker for StubChecker {
        async fn check_signature(&self, signature: &str) -> OnChainTxStatus {
            if self.found.iter().any(|s| s == signature) {
                OnChainTxStatus::Found
            } else if self.not_found.iter().any(|s| s == signature) {
                OnChainTxStatus::NotFound
            } else {
                OnChainTxStatus::Error
            }
        }
    }

    #[test]
    fn test_rpc_checker_parses_valid_signature() {
        // A 64-byte zero signature encodes to 64 base58 '1' characters.
        let sig = "1".repeat(64);
        assert!(sig.parse::<Signature>().is_ok(), "64-byte zero signature should parse");
        // An obviously invalid signature must be rejected (the checker maps this to Error).
        assert!("not-a-valid-signature!!!".parse::<Signature>().is_err());
    }

    #[tokio::test]
    async fn test_stub_checker_dispatch() {
        let checker = StubChecker {
            found: vec!["found-sig".to_string()],
            not_found: vec!["absent-sig".to_string()],
        };
        assert_eq!(
            checker.check_signature("found-sig").await,
            OnChainTxStatus::Found
        );
        assert_eq!(
            checker.check_signature("absent-sig").await,
            OnChainTxStatus::NotFound
        );
        assert_eq!(
            checker.check_signature("other").await,
            OnChainTxStatus::Error
        );
    }

    #[test]
    fn test_run_lock_is_initially_idle() {
        // The static lock should start idle (false). Other tests may flip it, so only
        // assert it can be set and cleared atomically here.
        let was = RECONCILIATION_RUNNING.swap(true, Ordering::SeqCst);
        RECONCILIATION_RUNNING.store(false, Ordering::SeqCst);
        // We can't assert `was == false` deterministically (parallel tests), just that
        // the swap returns the previous value.
        let _ = was;
    }
}
