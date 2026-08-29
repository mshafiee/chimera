//! Shadow recording-gap alarm (2026-08-22, profitability fix Phase 2).
//!
//! The shadow-measurement loop validates every selection/exit change, but a
//! silent recording gap makes it lie by omission: the write-time dedup in
//! `shadow_trader.rs` used to swallow ADMITTED signals whenever an earlier
//! REJECTED signal had already opened the (wallet, token) twin — the
//! gate-report ADMITTED row measured empty/wrong while decisions were live.
//!
//! This task periodically counts admitted BUY decision records from the
//! trailing 24h that have NO linked `shadow_positions` row and raises an
//! operator notification when the gap is sustained (> 0 on two consecutive
//! checks), rate-limited to one alert per hour until it clears.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::db_abstraction::Database;
use crate::notifications::{CompositeNotifier, NotificationEvent};

/// Admitted decisions without shadow coverage over the trailing window.
///
/// A decision counts as covered when EITHER a shadow row links to its own
/// decision_id, OR an ADMITTED twin row for the same (wallet, token) pair
/// was opened within the write-time dedup window (shadow_trader's
/// DEDUP_WINDOW_SECS = 1h): duplicate whale buys inside the window keep the
/// first row, so the pair is measured once and those decisions must not
/// alarm. A REJECTED twin does NOT cover — that is the original silent-gap
/// bug class (admitted gate-report measured by a rejected twin), which the
/// writer repairs on admission; if the repair fails this probe still fires.
async fn count_missing_shadow_rows(db: &Arc<dyn Database>) -> anyhow::Result<i64> {
    use crate::db_abstraction::DbPool;
    let DbPool::PostgreSQL(pool) = db.pool();
    let missing: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*)
           FROM decision_records dr
           WHERE dr.admitted = TRUE
             AND dr.action = 'BUY'
             AND dr.decided_at > NOW() - INTERVAL '24 hours'
             AND NOT EXISTS (
                 SELECT 1 FROM shadow_positions sp
                 WHERE sp.decision_id = dr.decision_id
                    OR (
                        sp.wallet_address = dr.wallet_address
                        AND sp.token_address = dr.token_address
                        AND sp.main_admitted = TRUE
                        AND sp.opened_at >= dr.decided_at - INTERVAL '1 hour'
                        AND sp.opened_at <= dr.decided_at + INTERVAL '1 hour'
                    )
             )"#,
    )
    .fetch_one(&pool)
    .await?;
    Ok(missing)
}

/// PROVING pool size and how many provers produced at least one decision in
/// the trailing 24h. Zero decisions from a populated pool is the 2026-08-28
/// cache-starve signature (12h of provers trading with zero evidence).
async fn count_proving_decisions_24h(db: &Arc<dyn Database>) -> anyhow::Result<(i64, i64)> {
    use crate::db_abstraction::DbPool;
    let DbPool::PostgreSQL(pool) = db.pool();
    let row: (i64, i64) = sqlx::query_as(
        r#"SELECT
             (SELECT COUNT(*) FROM wallets WHERE status = 'PROVING'),
             (SELECT COUNT(DISTINCT dr.wallet_address)
              FROM decision_records dr
              JOIN wallets w ON w.address = dr.wallet_address
              WHERE w.status = 'PROVING'
                AND dr.received_at > NOW() - INTERVAL '24 hours')"#,
    )
    .fetch_one(&pool)
    .await?;
    Ok(row)
}

/// Spawned alarm loop. `check_interval_secs` controls both cadence and how
/// fast a sustained gap escalates (two consecutive positive checks).
pub async fn start_shadow_gap_alarm(
    db: Arc<dyn Database>,
    notifier: Arc<CompositeNotifier>,
    check_interval_secs: u64,
    cancel_token: CancellationToken,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(check_interval_secs));
    info!(
        interval_secs = check_interval_secs,
        "Shadow recording-gap alarm started"
    );

    // Sustained-gap state: the gap must persist across two consecutive
    // checks before alerting, then re-alerts at most hourly until cleared.
    let mut consecutive_positive: u32 = 0;
    let mut last_alert: Option<tokio::time::Instant> = None;
    let realert_after = Duration::from_secs(3600);

    // Proving-lane starvation state: mirrors the gap machinery — a populated
    // PROVING pool with zero decisions in 24h must persist across two
    // consecutive checks before alerting, then re-alerts hourly until
    // decisions resume.
    let mut consecutive_positive_starved: u32 = 0;
    let mut last_alert_starved: Option<tokio::time::Instant> = None;

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                info!("Shadow recording-gap alarm cancelled");
                break;
            }
            _ = interval.tick() => {
                match count_missing_shadow_rows(&db).await {
                    Ok(missing) => {
                        if missing > 0 {
                            consecutive_positive = consecutive_positive.saturating_add(1);
                            let sustained = consecutive_positive >= 2;
                            let due = last_alert
                                .map(|t| t.elapsed() >= realert_after)
                                .unwrap_or(true);
                            if sustained && due {
                                error!(
                                    missing,
                                    "Shadow recording gap: admitted decisions without shadow positions"
                                );
                                notifier
                                    .notify(NotificationEvent::ShadowRecordingGap { missing })
                                    .await;
                                last_alert = Some(tokio::time::Instant::now());
                            } else if !sustained {
                                warn!(
                                    missing,
                                    "Shadow recording gap detected — awaiting second consecutive check"
                                );
                            }
                        } else {
                            if consecutive_positive > 0 {
                                info!("Shadow recording gap cleared");
                            }
                            consecutive_positive = 0;
                            last_alert = None;
                        }

                        match count_proving_decisions_24h(&db).await {
                            Ok((provers, with_decisions)) => {
                                let starved = provers >= 5 && with_decisions == 0;
                                if starved {
                                    consecutive_positive_starved =
                                        consecutive_positive_starved.saturating_add(1);
                                    let sustained = consecutive_positive_starved >= 2;
                                    let due = last_alert_starved
                                        .map(|t| t.elapsed() >= realert_after)
                                        .unwrap_or(true);
                                    if sustained && due {
                                        error!(
                                            provers,
                                            with_decisions,
                                            "Proving lane starved: zero decisions from populated pool in 24h"
                                        );
                                        notifier
                                            .notify(NotificationEvent::ProvingLaneStarved {
                                                provers,
                                                with_decisions,
                                            })
                                            .await;
                                        last_alert_starved = Some(tokio::time::Instant::now());
                                    }
                                } else {
                                    if consecutive_positive_starved > 0 {
                                        info!("Proving lane starvation cleared");
                                    }
                                    consecutive_positive_starved = 0;
                                    last_alert_starved = None;
                                }
                            }
                            Err(e) => {
                                warn!(error = %e, "Proving-lane starvation probe failed");
                            }
                        }
                    }
                    Err(e) => {
                        // A broken probe must not spam alerts; log and retry
                        // next tick.
                        warn!(error = %e, "Shadow recording-gap probe failed");
                    }
                }
            }
        }
    }
}
