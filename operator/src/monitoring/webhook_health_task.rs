//! Webhook Health Monitoring Task
//!
//! Continuous background task for webhook health monitoring,
//! reconciliation, and automatic cleanup of stale/orphaned webhooks.

use crate::db::{self, DbPool};
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
}

/// Start the webhook health monitoring task
///
/// This task runs continuously in the background, performing:
/// 1. Webhook reconciliation (detect and fix orphaned/missing webhooks)
/// 2. Health checks on all active webhooks
/// 3. Cleanup of stale/unhealthy webhooks
/// 4. URL change detection and bulk updates
pub async fn start_webhook_health_task(
    db: DbPool,
    helius_client: Arc<HeliusClient>,
    rate_limiter: Arc<RateLimiter>,
    config: WebhookHealthConfig,
    cancel_token: CancellationToken,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(config.check_interval_secs));

    // Create webhook lifecycle manager
    let lifecycle_config = WebhookLifecycleConfig {
        auto_register_enabled: true,
        auto_cleanup_enabled: true,
        health_check_interval_secs: config.check_interval_secs,
        stale_threshold_days: config.stale_threshold_days,
        max_registration_retries: 3,
        webhook_url: config.webhook_url.clone(),
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
                if let Err(e) = db::update_webhook_configuration(
                    &db,
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
    db: &DbPool,
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
    };

    let manager = WebhookLifecycleManager::new(
        db.clone(),
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
    db: &DbPool,
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
    };

    let manager = WebhookLifecycleManager::new(
        db.clone(),
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
pub async fn get_webhook_statistics(db: &DbPool) -> Result<crate::db::WebhookStats> {
    let stats = db::get_webhook_stats(db).await?;
    Ok(stats)
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
    db: DbPool,
    helius_client: Arc<HeliusClient>,
    rate_limiter: Arc<RateLimiter>,
    config: WebhookHealthConfig,
) -> Result<StartupWebhookResult> {
    let start = std::time::Instant::now();

    info!("Starting webhook check on startup");

    let lifecycle_config = WebhookLifecycleConfig {
        auto_register_enabled: true,
        auto_cleanup_enabled: config.stale_threshold_days > 0,
        health_check_interval_secs: config.check_interval_secs,
        stale_threshold_days: config.stale_threshold_days,
        max_registration_retries: 3,
        webhook_url: config.webhook_url.clone(),
    };

    let manager = WebhookLifecycleManager::new(
        db.clone(),
        helius_client,
        rate_limiter,
        lifecycle_config,
    );

    let mut registered = 0;
    let mut orphaned = 0;
    let mut cleaned_up = 0;
    let mut failed = 0;

    // 1. Register webhooks for ACTIVE wallets that need them
    let wallets_needing_webhooks = db::get_wallets_needing_webhook_registration(&db).await?;
    let wallets_checked = wallets_needing_webhooks.len();

    info!(
        wallets_count = wallets_checked,
        "Found ACTIVE wallets needing webhook registration"
    );

    for wallet_address in &wallets_needing_webhooks {
        match manager.register_wallet_webhook(wallet_address).await {
            Ok(result) if result.success => {
                registered += 1;
                info!(wallet = %wallet_address, webhook_id = %result.webhook_id, "Registered webhook on startup");
            }
            Ok(result) => {
                failed += 1;
                warn!(
                    wallet = %wallet_address,
                    error = ?result.error_message,
                    "Failed to register webhook on startup"
                );
            }
            Err(e) => {
                failed += 1;
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
    if config.stale_threshold_days > 0 {
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
        registered,
        orphaned,
        cleaned_up,
        failed,
        duration_ms,
        "Startup webhook check completed"
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
    db: DbPool,
    helius_client: Arc<HeliusClient>,
    rate_limiter: Arc<RateLimiter>,
    config: WebhookHealthConfig,
) -> Result<WebhookReconciliationResult> {
    let start = std::time::Instant::now();

    info!("Starting Helius webhook reconciliation (async background task)");

    let lifecycle_config = WebhookLifecycleConfig {
        auto_register_enabled: false,  // Don't register, only assess profitability
        auto_cleanup_enabled: true,
        health_check_interval_secs: config.check_interval_secs,
        stale_threshold_days: config.stale_threshold_days,
        max_registration_retries: 3,
        webhook_url: config.webhook_url.clone(),
    };

    let manager = WebhookLifecycleManager::new(
        db,
        helius_client,
        rate_limiter,
        lifecycle_config,
    );

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
        };

        assert_eq!(config.check_interval_secs, 3600);
        assert_eq!(config.stale_threshold_days, 7);
    }
}
