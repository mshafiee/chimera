//! Webhook Health Monitoring Task
//!
//! Continuous background task for webhook health monitoring,
//! reconciliation, and automatic cleanup of stale/orphaned webhooks.

use crate::db_abstraction::Database;
use crate::monitoring::helius::{HeliusClient, WebhookReconciliationResult};
use crate::monitoring::rate_limiter::RateLimiter;
use crate::monitoring::webhook_lifecycle::{WebhookLifecycleConfig, WebhookLifecycleManager};
use anyhow::Result;
use std::sync::Arc;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// Webhook health monitoring configuration
#[derive(Debug, Clone)]
pub struct WebhookHealthConfig {
    pub check_interval_secs: u64,
    pub stale_threshold_days: u32,
    pub webhook_url: String,
    pub helius_dry_run: bool,
    pub auto_cleanup_enabled: bool,
    pub auth_header: Option<String>,
}

/// `webhook_configuration` key holding a Unix-epoch seconds value; while
/// now < value, registration attempts are skipped. Set on quota-classified
/// registration failures (2026-08-15) so the health task stops hammering the
/// Helius webhook API with hundreds of doomed calls per cycle (441 failures /
/// 2 days) that waste the post-reset window and trigger Cloudflare 1015
/// rate limits.
const PAUSE_KEY: &str = "webhook_registration_paused_until";

/// Backoff duration after a quota-classified registration failure.
const PAUSE_SECS: u64 = 3600;

/// Returns true when registration is currently paused (cooldown active).
async fn registration_paused(db: &dyn Database) -> bool {
    match db.get_webhook_configuration(PAUSE_KEY).await {
        Ok(Some(ts)) => {
            let until: i64 = ts.trim().parse().unwrap_or(0);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            now < until
        }
        _ => false,
    }
}

/// Pauses registration for `PAUSE_SECS`. Only called on quota/rate-limit
/// classified failures; success or non-quota errors do not pause.
async fn pause_registration(db: &dyn Database, reason: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let until = now + PAUSE_SECS as i64;
    match db
        .update_webhook_configuration(PAUSE_KEY, &until.to_string(), "health_task")
        .await
    {
        Ok(_) => info!(
            paused_until = until,
            reason = %reason,
            "Webhook registration paused (quota/rate-limit backoff)"
        ),
        Err(e) => warn!(error = %e, "Failed to persist webhook registration pause marker"),
    }
}

/// Classifies a registration error message as quota/rate-limit (pauses
/// registration) vs. transient per-wallet (does not).
fn is_quota_classified(msg: &str) -> bool {
    let low = msg.to_ascii_lowercase();
    low.contains("max usage reached")
        || low.contains("1015")
        || low.contains("429")
        || low.contains("rate limit")
}

/// Start the webhook health monitoring task
///
/// This task runs continuously in the background, performing:
/// 1. Webhook reconciliation (detect and fix orphaned/missing webhooks)
/// 2. Health checks on all active webhooks
/// 3. Cleanup of stale/unhealthy webhooks
/// 4. URL change detection and bulk updates
pub async fn start_webhook_health_task(
    db: Arc<dyn Database>,
    helius_client: Arc<HeliusClient>,
    rate_limiter: Arc<RateLimiter>,
    config: WebhookHealthConfig,
    cancel_token: CancellationToken,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(config.check_interval_secs));

    // Create webhook lifecycle manager
    let lifecycle_config = WebhookLifecycleConfig {
        auto_register_enabled: true,
        auto_cleanup_enabled: config.auto_cleanup_enabled,
        health_check_interval_secs: config.check_interval_secs,
        stale_threshold_days: config.stale_threshold_days,
        max_registration_retries: 3,
        webhook_url: config.webhook_url.clone(),
        helius_dry_run: config.helius_dry_run,
        auth_header: config.auth_header.clone(),
    };

    let manager = WebhookLifecycleManager::new(
        db.clone(),
        helius_client.clone(),
        rate_limiter.clone(),
        lifecycle_config,
    );

    info!(
        interval_secs = config.check_interval_secs,
        stale_days = config.stale_threshold_days,
        "Webhook health monitoring task started"
    );

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                info!("Webhook health monitoring task cancelled");
                break;
            }
            _ = interval.tick() => {
                info!("Starting webhook health check cycle");

                // 1. Reconcile webhooks with Helius
                match manager.reconcile_webhooks().await {
                    Ok(stats) => {
                        info!(
                            registered = stats.registered,
                            orphaned = stats.orphaned,
                            updated = stats.updated,
                            failed = stats.failed,
                            duration_ms = stats.duration_ms,
                            "Webhook reconciliation completed"
                        );
                    }
                    Err(e) => {
                        error!(error = %e, "Webhook reconciliation failed");
                    }
                }

                // 2. Cleanup stale webhooks
                match manager.health_check_webhooks().await {
                    Ok(stats) => {
                        info!(
                            total_checked = stats.total_checked,
                            healthy = stats.healthy,
                            unhealthy = stats.unhealthy,
                            cleaned_up = stats.cleaned_up,
                            duration_ms = stats.duration_ms,
                            "Webhook health check completed"
                        );
                    }
                    Err(e) => {
                        error!(error = %e, "Webhook health check failed");
                    }
                }

                // 3. Update configuration tracking
                if let Err(e) = db.update_webhook_configuration(
                    "current_webhook_url",
                    &config.webhook_url,
                    "health_task"
                ).await {
                    warn!(error = %e, "Failed to update webhook configuration tracking");
                }

                info!("Webhook health check cycle completed");
            }
        }
    }

    info!("Webhook health monitoring task stopped");
}

/// Manual trigger for webhook reconciliation
///
/// This can be called via API endpoint to manually trigger
/// webhook reconciliation outside of the scheduled interval.
pub async fn manual_reconcile_webhooks(
    db: Arc<dyn Database>,
    helius_client: &Arc<HeliusClient>,
    rate_limiter: &Arc<RateLimiter>,
    webhook_url: &str,
) -> Result<crate::monitoring::webhook_lifecycle::ReconciliationResult> {
    info!("Manual webhook reconciliation triggered");

    let lifecycle_config = WebhookLifecycleConfig {
        auto_register_enabled: true,
        auto_cleanup_enabled: true,
        health_check_interval_secs: 3600,
        stale_threshold_days: 7,
        max_registration_retries: 3,
        webhook_url: webhook_url.to_string(),
        helius_dry_run: true,
        auth_header: None,
    };

    let manager = WebhookLifecycleManager::new(
        db,
        helius_client.clone(),
        rate_limiter.clone(),
        lifecycle_config,
    );

    let result = manager.reconcile_webhooks().await?;

    info!(
        registered = result.registered,
        orphaned = result.orphaned,
        updated = result.updated,
        failed = result.failed,
        "Manual webhook reconciliation completed"
    );

    Ok(result)
}

/// Manual trigger for webhook health check
///
/// This can be called via API endpoint to manually trigger
/// webhook health checks outside of the scheduled interval.
pub async fn manual_health_check(
    db: Arc<dyn Database>,
    helius_client: &Arc<HeliusClient>,
    rate_limiter: &Arc<RateLimiter>,
    webhook_url: &str,
    stale_threshold_days: u32,
) -> Result<crate::monitoring::webhook_lifecycle::HealthCheckResult> {
    info!("Manual webhook health check triggered");

    let lifecycle_config = WebhookLifecycleConfig {
        auto_register_enabled: true,
        auto_cleanup_enabled: true,
        health_check_interval_secs: 3600,
        stale_threshold_days,
        max_registration_retries: 3,
        webhook_url: webhook_url.to_string(),
        helius_dry_run: true,
        auth_header: None,
    };

    let manager = WebhookLifecycleManager::new(
        db,
        helius_client.clone(),
        rate_limiter.clone(),
        lifecycle_config,
    );

    let result = manager.health_check_webhooks().await?;

    info!(
        total_checked = result.total_checked,
        healthy = result.healthy,
        unhealthy = result.unhealthy,
        cleaned_up = result.cleaned_up,
        "Manual webhook health check completed"
    );

    Ok(result)
}

/// Get webhook statistics for monitoring
pub async fn get_webhook_statistics(
    db: &dyn Database,
) -> Result<crate::db_abstraction::WebhookStats> {
    let all_monitoring = db.get_all_wallet_monitoring().await?;
    let total = all_monitoring.len();
    let active = all_monitoring
        .iter()
        .filter(|m| m.webhook_status.as_deref() == Some("active"))
        .count();
    let failed = all_monitoring
        .iter()
        .filter(|m| {
            m.webhook_health_status.as_deref() == Some("error")
                || m.webhook_health_status.as_deref() == Some("unhealthy")
        })
        .count();
    Ok(crate::db_abstraction::WebhookStats {
        total_webhooks: total,
        active_webhooks: active,
        stale_webhooks: 0,
        failed_registrations: failed,
    })
}

/// Startup webhook check result
#[derive(Debug, Clone)]
pub struct StartupWebhookResult {
    pub wallets_checked: usize,
    pub registered: usize,
    pub orphaned: usize,
    pub cleaned_up: usize,
    pub failed: usize,
    pub duration_ms: u64,
}

/// Run webhook management check on startup
///
/// This function runs once during operator startup to ensure all
/// ACTIVE wallets have registered webhooks. It performs:
/// 1. Registration for wallets missing webhooks
/// 2. Cleanup of orphaned webhooks
/// 3. Optional cleanup of stale webhooks
pub async fn run_startup_webhook_check(
    db: Arc<dyn Database>,
    helius_client: Arc<HeliusClient>,
    rate_limiter: Arc<RateLimiter>,
    config: WebhookHealthConfig,
) -> Result<StartupWebhookResult> {
    let start = std::time::Instant::now();

    info!("Starting webhook check on startup");

    let lifecycle_config = WebhookLifecycleConfig {
        auto_register_enabled: true,
        auto_cleanup_enabled: config.auto_cleanup_enabled,
        health_check_interval_secs: config.check_interval_secs,
        stale_threshold_days: config.stale_threshold_days,
        max_registration_retries: 3,
        webhook_url: config.webhook_url.clone(),
        helius_dry_run: config.helius_dry_run,
        auth_header: config.auth_header.clone(),
    };

    let manager =
        WebhookLifecycleManager::new(db.clone(), helius_client, rate_limiter, lifecycle_config);

    let mut registered = 0;
    let mut orphaned = 0;
    let mut cleaned_up = 0;
    let mut failed = 0;

    // 0. Verify existing webhook IDs against Helius — clear stale ones so they get re-registered.
    // This fixes the case where the DB has a webhook_id but the webhook was deleted from Helius
    // (e.g., manually or by a previous cleanup), leaving monitoring silently broken.
    let wallets_with_ids = db.get_active_wallets_with_webhook_ids().await.unwrap_or_default();
    if !wallets_with_ids.is_empty() {
        match manager.get_helius_webhook_ids().await {
            Ok(helius_webhook_ids) => {
                let mut stale_count = 0;
                for (wallet_address, stored_id) in &wallets_with_ids {
                    if !helius_webhook_ids.contains(stored_id) {
                        info!(
                            wallet = %wallet_address,
                            stored_webhook_id = %stored_id,
                            "Webhook ID in DB not found in Helius — clearing for re-registration"
                        );
                        match db.clear_webhook_id(wallet_address).await {
                            Ok(_) => stale_count += 1,
                            Err(e) => {
                                error!(
                                    wallet = %wallet_address,
                                    error = %e,
                                    "Failed to clear stale webhook_id from database"
                                );
                            }
                        }
                    }
                }
                if stale_count > 0 {
                    info!(count = stale_count, "Cleared stale webhook IDs from database");
                }
            }
            Err(e) => {
                warn!(error = %e, "Could not verify webhook IDs against Helius — skipping staleness check");
            }
        }
    }

    // 1. Register webhooks for ACTIVE wallets that need them (re-query after clearing stale IDs)
    // Backoff (2026-08-15): when a prior cycle hit a quota/rate-limit error,
    // skip registration entirely until the pause expires. The old behavior
    // retried hundreds of doomed calls per cycle, wasting the post-reset
    // window and triggering Cloudflare 1015 rate limits that blocked even
    // legitimate retries.
    let wallets_needing_webhooks = db.get_wallets_needing_webhook_registration().await?;
    let wallets_checked = wallets_needing_webhooks.len();

    if wallets_checked > 0 {
        info!(
            wallets_count = wallets_checked,
            "Found ACTIVE wallets needing webhook registration"
        );
    }

    if registration_paused(db.as_ref()).await {
        info!(
            wallets_skipped = wallets_checked,
            "Webhook registration skipped — cooldown active from a prior quota/rate-limit failure"
        );
        return Ok(StartupWebhookResult {
            wallets_checked,
            registered: 0,
            orphaned,
            cleaned_up,
            failed: 0,
            duration_ms: start.elapsed().as_millis() as u64,
        });
    }

    for wallet_address in &wallets_needing_webhooks {
        match manager.register_wallet_webhook(wallet_address).await {
            Ok(result) if result.success => {
                registered += 1;
                info!(wallet = %wallet_address, webhook_id = %result.webhook_id, "Registered webhook on startup");
            }
            Ok(result) => {
                failed += 1;
                let err_msg = result.error_message.clone().unwrap_or_default();
                if is_quota_classified(&err_msg) {
                    pause_registration(db.as_ref(), &err_msg).await;
                }
                warn!(
                    wallet = %wallet_address,
                    error = ?result.error_message,
                    "Failed to register webhook on startup"
                );
            }
            Err(e) => {
                failed += 1;
                if is_quota_classified(&e.to_string()) {
                    pause_registration(db.as_ref(), &e.to_string()).await;
                }
                error!(
                    wallet = %wallet_address,
                    error = %e,
                    "Error registering webhook on startup"
                );
            }
        }
    }

    // 2. Detect and clean up orphaned webhooks (exist in Helius but not our ACTIVE wallets)
    match manager.reconcile_webhooks().await {
        Ok(reconcile_result) => {
            orphaned = reconcile_result.orphaned;
            if orphaned > 0 {
                info!(count = orphaned, "Cleaned up orphaned webhooks on startup");
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed to reconcile webhooks on startup");
        }
    }

    // 3. Cleanup stale webhooks if enabled
    if config.auto_cleanup_enabled {
        match manager.health_check_webhooks().await {
            Ok(health_result) => {
                cleaned_up = health_result.cleaned_up;
                if cleaned_up > 0 {
                    info!(count = cleaned_up, "Cleaned up stale webhooks on startup");
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to run health check on startup");
            }
        }
    }

    let duration_ms = start.elapsed().as_millis() as u64;

    info!(
        wallets_checked,
        registered, orphaned, cleaned_up, failed, duration_ms, "Startup webhook check completed"
    );

    Ok(StartupWebhookResult {
        wallets_checked,
        registered,
        orphaned,
        cleaned_up,
        failed,
        duration_ms,
    })
}

/// Reconcile webhooks with Helius dashboard (async/background friendly)
///
/// This function is designed to run as a background task via tokio::spawn.
/// It discovers all webhooks in Helius dashboard and assesses profitability
/// of each wallet to determine which webhooks to keep or delete.
pub async fn reconcile_helius_webhooks_async(
    db: Arc<dyn Database>,
    helius_client: Arc<HeliusClient>,
    rate_limiter: Arc<RateLimiter>,
    config: WebhookHealthConfig,
) -> Result<WebhookReconciliationResult> {
    let start = std::time::Instant::now();

    info!("Starting Helius webhook reconciliation (async background task)");

    let lifecycle_config = WebhookLifecycleConfig {
        auto_register_enabled: false, // Don't register, only assess profitability
        auto_cleanup_enabled: config.auto_cleanup_enabled,
        health_check_interval_secs: config.check_interval_secs,
        stale_threshold_days: config.stale_threshold_days,
        max_registration_retries: 3,
        webhook_url: config.webhook_url.clone(),
        helius_dry_run: config.helius_dry_run,
        auth_header: config.auth_header.clone(),
    };

    let manager = WebhookLifecycleManager::new(db, helius_client, rate_limiter, lifecycle_config);

    // Run reconciliation with profitability assessment
    let result = manager.reconcile_with_helius_dashboard().await?;

    let duration_ms = start.elapsed().as_millis() as u64;

    info!(
        total = result.total_helius_webhooks,
        eligible = result.eligible_wallets,
        ineligible = result.ineligible_wallets,
        deleted = result.deleted_webhooks,
        duration_ms,
        "Helius webhook reconciliation completed"
    );

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_webhook_health_config() {
        let config = WebhookHealthConfig {
            check_interval_secs: 3600,
            stale_threshold_days: 7,
            webhook_url: "https://example.com/webhook".to_string(),
            helius_dry_run: true,
            auto_cleanup_enabled: false,
            auth_header: None,
        };

        assert_eq!(config.check_interval_secs, 3600);
        assert_eq!(config.stale_threshold_days, 7);
        assert!(config.helius_dry_run);
        assert!(!config.auto_cleanup_enabled);
    }

    #[test]
    fn test_is_quota_classified_recognizes_quota_and_rate_limits() {
        // The exact strings observed in production during the daily-quota
        // death spiral (2026-08-15).
        assert!(is_quota_classified(
            "Webhook registration failed: {\"jsonrpc\":\"2.0\",\"error\":{\"code\":-32429,\"message\":\"max usage reached\"}}"
        ));
        assert!(is_quota_classified("Webhook registration failed: error code: 1015"));
        assert!(is_quota_classified("HTTP 429 Too Many Requests"));
        assert!(is_quota_classified("rate limit exceeded"));

        // Non-quota errors must NOT pause registration.
        assert!(!is_quota_classified("Invalid Solana address format"));
        assert!(!is_quota_classified("Webhook ID no longer exists in Helius — clearing"));
        assert!(!is_quota_classified("connection reset by peer"));
        assert!(!is_quota_classified(""));
    }
}
