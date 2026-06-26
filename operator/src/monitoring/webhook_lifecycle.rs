//! Webhook Lifecycle Management Module
//!
//! Provides comprehensive webhook registration, monitoring, and cleanup
//! for ACTIVE wallets with automatic health checking and reconciliation.

use crate::db_abstraction::{Database, WebhookEligibility};
use crate::monitoring::helius::{HeliusClient, WebhookReconciliationDetail, WebhookReconciliationResult, WebhookUpdate};
use crate::monitoring::rate_limiter::{RateLimiter, RequestPriority};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{error, info, warn};

/// Webhook lifecycle configuration
#[derive(Debug, Clone)]
pub struct WebhookLifecycleConfig {
    pub auto_register_enabled: bool,
    pub auto_cleanup_enabled: bool,
    pub health_check_interval_secs: u64,
    pub stale_threshold_days: u32,
    pub max_registration_retries: u32,
    pub webhook_url: String,
}

/// Webhook registration result
#[derive(Debug, Serialize, Deserialize)]
pub struct WebhookRegistrationResult {
    pub wallet_address: String,
    pub webhook_id: String,
    pub success: bool,
    pub error_message: Option<String>,
    pub duration_ms: i32,
}

/// Bulk operation result
#[derive(Debug, Serialize, Deserialize)]
pub struct BulkOperationResult {
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub results: Vec<WalletOperationResult>,
    pub duration_ms: i32,
}

/// Individual wallet operation result
#[derive(Debug, Serialize, Deserialize)]
pub struct WalletOperationResult {
    pub wallet_address: String,
    pub success: bool,
    pub error_message: Option<String>,
    pub duration_ms: i32,
}

/// Reconciliation statistics
#[derive(Debug, Serialize, Deserialize)]
pub struct ReconciliationResult {
    pub registered: usize,
    pub orphaned: usize,
    pub updated: usize,
    pub failed: usize,
    pub duration_ms: i32,
}

/// Health check result
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthCheckResult {
    pub total_checked: usize,
    pub healthy: usize,
    pub unhealthy: usize,
    pub cleaned_up: usize,
    pub duration_ms: i32,
}

/// Webhook lifecycle manager
pub struct WebhookLifecycleManager {
    db: Arc<dyn Database>,
    helius_client: Arc<HeliusClient>,
    rate_limiter: Arc<RateLimiter>,
    config: WebhookLifecycleConfig,
}

impl WebhookLifecycleManager {
    pub fn new(
        db: Arc<dyn Database>,
        helius_client: Arc<HeliusClient>,
        rate_limiter: Arc<RateLimiter>,
        config: WebhookLifecycleConfig,
    ) -> Self {
        Self {
            db,
            helius_client,
            rate_limiter,
            config,
        }
    }

    /// Register webhook for newly promoted wallet with comprehensive error handling
    pub async fn register_wallet_webhook(&self, wallet: &str) -> Result<WebhookRegistrationResult> {
        let start = Instant::now();
        let wallet = wallet.trim();

        info!(wallet = %wallet, "Starting webhook registration");

        // Validate wallet address format
        if !self.is_valid_solana_address(wallet) {
            let error = "Invalid Solana address format".to_string();
            error!(wallet = %wallet, error = %error, "Webhook registration failed");
            return Ok(WebhookRegistrationResult {
                wallet_address: wallet.to_string(),
                webhook_id: String::new(),
                success: false,
                error_message: Some(error),
                duration_ms: start.elapsed().as_millis() as i32,
            });
        }

        // Check if webhook already exists
        if let Ok(Some(existing_webhook)) = self.db.get_wallet_monitoring(wallet).await {
            if let Some(webhook_id) = &existing_webhook.helius_webhook_id {
                if !webhook_id.is_empty() {
                    info!(
                        wallet = %wallet,
                        webhook_id = %webhook_id,
                        "Webhook already exists"
                    );
                    return Ok(WebhookRegistrationResult {
                        wallet_address: wallet.to_string(),
                        webhook_id: webhook_id.clone(),
                        success: true,
                        error_message: None,
                        duration_ms: start.elapsed().as_millis() as i32,
                    });
                }
            }
        }

        // Rate limit before API call
        self.rate_limiter
            .acquire_standard(RequestPriority::Polling)
            .await;

        // Register webhook with Helius
        match self
            .helius_client
            .register_webhook(&[wallet.to_string()], &self.config.webhook_url)
            .await
        {
            Ok(webhook_id) => {
                // Update database with webhook ID
                if let Err(e) = self.db.upsert_wallet_monitoring(wallet, Some(&webhook_id), true).await {
                    warn!(wallet = %wallet, error = %e, "Database update failed, but webhook registered");
                }

                // Log successful registration
                let _ = self.db.log_webhook_lifecycle_event(
                    wallet,
                    "register",
                    "success",
                    Some(&webhook_id),
                    Some(&format!("Registered webhook for wallet {}", wallet)),
                    None,
                    Some(start.elapsed().as_millis() as i32),
                ).await;

                info!(
                    wallet = %wallet,
                    webhook_id = %webhook_id,
                    "Webhook registration successful"
                );
                Ok(WebhookRegistrationResult {
                    wallet_address: wallet.to_string(),
                    webhook_id,
                    success: true,
                    error_message: None,
                    duration_ms: start.elapsed().as_millis() as i32,
                })
            }
            Err(e) => {
                // Increment retry count
                let _ = self.db.increment_webhook_registration_attempts(wallet, Some(&e.to_string())).await;

                // Log failed registration
                let _ = self.db.log_webhook_lifecycle_event(
                    wallet,
                    "register",
                    "failed",
                    None,
                    None,
                    Some(&e.to_string()),
                    Some(start.elapsed().as_millis() as i32),
                ).await;

                error!(wallet = %wallet, error = %e, "Webhook registration failed");
                Ok(WebhookRegistrationResult {
                    wallet_address: wallet.to_string(),
                    webhook_id: String::new(),
                    success: false,
                    error_message: Some(e.to_string()),
                    duration_ms: start.elapsed().as_millis() as i32,
                })
            }
        }
    }

    /// Cleanup webhook for demoted wallet
    pub async fn cleanup_wallet_webhook(&self, wallet: &str) -> Result<()> {
        let start = Instant::now();
        info!(wallet = %wallet, "Starting webhook cleanup");

        // Get existing webhook ID
        let monitoring = self.db.get_wallet_monitoring(wallet)
            .await?
            .context("Wallet monitoring not found")?;

        let webhook_id = monitoring
            .helius_webhook_id
            .context("No webhook ID found for wallet")?;

        // Rate limit before API call
        self.rate_limiter
            .acquire_standard(RequestPriority::Polling)
            .await;

        // Delete webhook from Helius
        match self.helius_client.delete_webhook(&webhook_id).await {
            Ok(()) => {
                // Update database
                let _ = self.db.upsert_wallet_monitoring(wallet, None, false).await;

                // Log successful cleanup
                let _ = self.db.log_webhook_lifecycle_event(
                    wallet,
                    "delete",
                    "success",
                    Some(&webhook_id),
                    Some(&format!("Deleted webhook for wallet {}", wallet)),
                    None,
                    Some(start.elapsed().as_millis() as i32),
                ).await;

                info!(
                    wallet = %wallet,
                    webhook_id = %webhook_id,
                    "Webhook cleanup successful"
                );
                Ok(())
            }
            Err(e) => {
                // Log failed cleanup
                let _ = self.db.log_webhook_lifecycle_event(
                    wallet,
                    "delete",
                    "failed",
                    Some(&webhook_id),
                    None,
                    Some(&e.to_string()),
                    Some(start.elapsed().as_millis() as i32),
                ).await;

                error!(wallet = %wallet, error = %e, "Webhook cleanup failed");
                Err(e)
            }
        }
    }

    /// Update existing webhook URL without recreation
    pub async fn update_wallet_webhook(&self, wallet: &str, new_url: String) -> Result<()> {
        let start = Instant::now();
        info!(wallet = %wallet, new_url = %new_url, "Starting webhook update");

        // Get existing webhook ID
        let monitoring = self.db.get_wallet_monitoring(wallet)
            .await?
            .context("Wallet monitoring not found")?;

        let webhook_id = monitoring
            .helius_webhook_id
            .context("No webhook ID found for wallet")?;

        // Rate limit before API call
        self.rate_limiter
            .acquire_standard(RequestPriority::Polling)
            .await;

        // Update webhook URL
        match self
            .helius_client
            .update_webhook(&webhook_id, WebhookUpdate {
                webhook_url: Some(new_url.clone()),
                transaction_types: None,
                account_addresses: None,
                auth_header: None,
            })
            .await
        {
            Ok(()) => {
                // Track the webhook URL update via configuration
                let _ = self.db.update_webhook_configuration(
                    &format!("webhook_url:{}", wallet),
                    &new_url,
                    "update_wallet_webhook",
                ).await;

                // Log successful update
                let _ = self.db.log_webhook_lifecycle_event(
                    wallet,
                    "update",
                    "success",
                    Some(&webhook_id),
                    Some(&format!("Updated webhook URL to {}", new_url)),
                    None,
                    Some(start.elapsed().as_millis() as i32),
                ).await;

                info!(
                    wallet = %wallet,
                    webhook_id = %webhook_id,
                    "Webhook update successful"
                );
                Ok(())
            }
            Err(e) => {
                // Log failed update
                let _ = self.db.log_webhook_lifecycle_event(
                    wallet,
                    "update",
                    "failed",
                    Some(&webhook_id),
                    None,
                    Some(&e.to_string()),
                    Some(start.elapsed().as_millis() as i32),
                ).await;

                error!(wallet = %wallet, error = %e, "Webhook update failed");
                Err(e)
            }
        }
    }

    /// Toggle webhook enable/disable without deletion
    pub async fn toggle_wallet_webhook(&self, wallet: &str, enabled: bool) -> Result<()> {
        let start = Instant::now();
        info!(wallet = %wallet, enabled = enabled, "Starting webhook toggle");

        // Get existing webhook ID
        let monitoring = self.db.get_wallet_monitoring(wallet)
            .await?
            .context("Wallet monitoring not found")?;

        let webhook_id = monitoring
            .helius_webhook_id
            .context("No webhook ID found for wallet")?;

        // Rate limit before API call
        self.rate_limiter
            .acquire_standard(RequestPriority::Polling)
            .await;

        // Toggle webhook
        match self.helius_client.toggle_webhook(&webhook_id, enabled).await {
            Ok(()) => {
                // Update database webhook status
                let status = if enabled { "active" } else { "paused" };
                let _ = self.db.update_webhook_status(wallet, status).await;

                // Log successful toggle
                let _ = self.db.log_webhook_lifecycle_event(
                    wallet,
                    "toggle",
                    "success",
                    Some(&webhook_id),
                    Some(&format!("Webhook toggled to {}", status)),
                    None,
                    Some(start.elapsed().as_millis() as i32),
                ).await;

                info!(
                    wallet = %wallet,
                    webhook_id = %webhook_id,
                    status = %status,
                    "Webhook toggle successful"
                );
                Ok(())
            }
            Err(e) => {
                // Log failed toggle
                let _ = self.db.log_webhook_lifecycle_event(
                    wallet,
                    "toggle",
                    "failed",
                    Some(webhook_id.as_str()),
                    None,
                    Some(&e.to_string()),
                    Some(start.elapsed().as_millis() as i32),
                ).await;

                error!(wallet = %wallet, error = %e, "Webhook toggle failed");
                Err(e)
            }
        }
    }

    /// Bulk register webhooks for multiple wallets with detailed results
    pub async fn bulk_register_webhooks(&self, wallets: Vec<String>) -> Result<BulkOperationResult> {
        let start = Instant::now();
        info!(count = wallets.len(), "Starting bulk webhook registration");

        let mut results = Vec::new();
        let mut succeeded = 0;
        let mut failed = 0;

        for wallet in wallets {
            let result = self.register_wallet_webhook(&wallet).await;
            match result {
                Ok(reg_result) if reg_result.success => {
                    succeeded += 1;
                    results.push(WalletOperationResult {
                        wallet_address: wallet,
                        success: true,
                        error_message: None,
                        duration_ms: reg_result.duration_ms,
                    });
                }
                Ok(reg_result) => {
                    failed += 1;
                    results.push(WalletOperationResult {
                        wallet_address: wallet,
                        success: false,
                        error_message: reg_result.error_message,
                        duration_ms: reg_result.duration_ms,
                    });
                }
                Err(e) => {
                    failed += 1;
                    results.push(WalletOperationResult {
                        wallet_address: wallet,
                        success: false,
                        error_message: Some(e.to_string()),
                        duration_ms: start.elapsed().as_millis() as i32,
                    });
                }
            }

            // Small delay between registrations to avoid rate limiting
            sleep(Duration::from_millis(100)).await;
        }

        info!(
            total = results.len(),
            succeeded = succeeded,
            failed = failed,
            duration_ms = start.elapsed().as_millis(),
            "Bulk webhook registration completed"
        );

        Ok(BulkOperationResult {
            total: results.len(),
            succeeded,
            failed,
            results,
            duration_ms: start.elapsed().as_millis() as i32,
        })
    }

    /// Bulk cleanup webhooks for multiple wallets
    pub async fn bulk_cleanup_webhooks(&self, wallets: Vec<String>) -> Result<BulkOperationResult> {
        let start = Instant::now();
        info!(count = wallets.len(), "Starting bulk webhook cleanup");

        let mut results = Vec::new();
        let mut succeeded = 0;
        let mut failed = 0;

        for wallet in wallets {
            let result = self.cleanup_wallet_webhook(&wallet).await;
            match result {
                Ok(()) => {
                    succeeded += 1;
                    results.push(WalletOperationResult {
                        wallet_address: wallet,
                        success: true,
                        error_message: None,
                        duration_ms: start.elapsed().as_millis() as i32,
                    });
                }
                Err(e) => {
                    failed += 1;
                    results.push(WalletOperationResult {
                        wallet_address: wallet,
                        success: false,
                        error_message: Some(e.to_string()),
                        duration_ms: start.elapsed().as_millis() as i32,
                    });
                }
            }

            // Small delay between cleanups to avoid rate limiting
            sleep(Duration::from_millis(100)).await;
        }

        info!(
            total = results.len(),
            succeeded = succeeded,
            failed = failed,
            duration_ms = start.elapsed().as_millis(),
            "Bulk webhook cleanup completed"
        );

        Ok(BulkOperationResult {
            total: results.len(),
            succeeded,
            failed,
            results,
            duration_ms: start.elapsed().as_millis() as i32,
        })
    }

    /// Reconcile database state with Helius webhooks
    pub async fn reconcile_webhooks(&self) -> Result<ReconciliationResult> {
        let start = Instant::now();
        info!("Starting webhook reconciliation");

        let mut registered = 0;
        let mut orphaned = 0;
        let mut updated = 0;
        let mut failed = 0;

        // Get all webhooks from Helius
        let helius_webhooks = self
            .helius_client
            .list_webhooks()
            .await
            .context("Failed to list Helius webhooks")?;

        // Get all webhooks from database
        let db_webhooks = self.db.get_all_wallet_monitoring()
            .await
            .context("Failed to get database webhooks")?;

        // Extract webhook IDs from Helius response
        let helius_webhook_ids: Vec<String> = helius_webhooks
            .iter()
            .filter_map(|hw| hw.get("webhookID").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .collect();

        // Find orphaned webhooks (in Helius but not in DB)
        let orphaned_webhooks: Vec<String> = helius_webhook_ids
            .iter()
            .filter(|hw_id| !db_webhooks.iter().any(|dw| {
                dw.helius_webhook_id
                    .as_ref()
                    .map(|id| id.as_str() == **hw_id)
                    .unwrap_or(false)
            }))
            .cloned()
            .collect();

        // Find missing webhooks (in DB but not in Helius)
        let missing_wallets: Vec<String> = db_webhooks
            .iter()
            .filter(|dw| dw.monitoring_enabled == 1 && dw.helius_webhook_id.is_none())
            .map(|dw| dw.wallet_address.clone())
            .collect();

        // Cleanup orphaned webhooks
        for webhook_id in orphaned_webhooks {
            match self.helius_client.delete_webhook(&webhook_id).await {
                Ok(()) => {
                    orphaned += 1;
                    info!(webhook_id = %webhook_id, "Cleaned up orphaned webhook");
                }
                Err(e) => {
                    failed += 1;
                    warn!(webhook_id = %webhook_id, error = %e, "Failed to cleanup orphaned webhook");
                }
            }
        }

        // Register missing webhooks
        for wallet in missing_wallets {
            match self.register_wallet_webhook(&wallet).await {
                Ok(result) if result.success => {
                    registered += 1;
                }
                Err(e) => {
                    failed += 1;
                    warn!(wallet = %wallet, error = %e, "Failed to register missing webhook");
                }
                _ => {}
            }
        }

        // Check for webhook URL changes
        if let Ok(Some(configured_url)) =
            self.db.get_webhook_configuration("current_webhook_url").await
        {
            if configured_url != self.config.webhook_url {
                info!("Webhook URL changed, updating all webhooks");
                let updates: Vec<(String, String)> = db_webhooks
                    .iter()
                    .filter_map(|dw| {
                        dw.helius_webhook_id.clone().map(|id| {
                            (id, self.config.webhook_url.clone())
                        })
                    })
                    .collect();

                match self
                    .helius_client
                    .bulk_update_webhook_urls(updates, self.rate_limiter.clone())
                    .await
                {
                    Ok(results) => {
                        updated = results.len();
                        // Update configuration
                        let _ = self.db.update_webhook_configuration(
                            "current_webhook_url",
                            &self.config.webhook_url,
                            "reconcile",
                        ).await;
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to bulk update webhook URLs");
                    }
                }
            }
        }

        info!(
            registered = registered,
            orphaned = orphaned,
            updated = updated,
            failed = failed,
            duration_ms = start.elapsed().as_millis(),
            "Webhook reconciliation completed"
        );

        Ok(ReconciliationResult {
            registered,
            orphaned,
            updated,
            failed,
            duration_ms: start.elapsed().as_millis() as i32,
        })
    }

    /// Health check and cleanup stale webhooks
    pub async fn health_check_webhooks(&self) -> Result<HealthCheckResult> {
        let start = Instant::now();
        info!("Starting webhook health check");

        // Get stale webhooks
        let stale_wallets =
            self.db.get_stale_webhook_wallets(self.config.stale_threshold_days as i32).await?;

        let total_checked = stale_wallets.len();
        let mut healthy = 0;
        let mut unhealthy = 0;
        let mut cleaned_up = 0;

        for wallet in &stale_wallets {
            // Check webhook health via Helius API
            match self.check_webhook_health(wallet).await {
                Ok(true) => {
                    healthy += 1;
                    let _ = self.db.update_webhook_health_status(wallet, "healthy", None).await;
                }
                Ok(false) => {
                    unhealthy += 1;
                    let _ =
                        self.db.update_webhook_health_status(wallet, "unhealthy", None).await;
                }
                Err(e) => {
                    warn!(wallet = %wallet, error = %e, "Webhook health check failed");
                    unhealthy += 1;
                    let _ = self.db.update_webhook_health_status(wallet, "error", None).await;
                }
            }
        }

        // Cleanup unhealthy webhooks if auto-cleanup enabled
        if self.config.auto_cleanup_enabled {
            let mut unhealthy_wallets = Vec::new();
            for wallet in &stale_wallets {
                // Check if webhook is unhealthy
                if let Ok(Some(monitoring)) = self.db.get_wallet_monitoring(wallet).await {
                    if monitoring.webhook_health_status.as_deref() == Some("unhealthy") {
                        unhealthy_wallets.push(wallet.clone());
                    }
                }
            }

            for wallet in unhealthy_wallets {
                match self.cleanup_wallet_webhook(&wallet).await {
                    Ok(()) => cleaned_up += 1,
                    Err(e) => warn!(wallet = %wallet, error = %e, "Failed to cleanup unhealthy webhook"),
                }
            }
        }

        info!(
            total_checked = total_checked,
            healthy = healthy,
            unhealthy = unhealthy,
            cleaned_up = cleaned_up,
            duration_ms = start.elapsed().as_millis(),
            "Webhook health check completed"
        );

        Ok(HealthCheckResult {
            total_checked,
            healthy,
            unhealthy,
            cleaned_up,
            duration_ms: start.elapsed().as_millis() as i32,
        })
    }

    /// Check individual webhook health
    async fn check_webhook_health(&self, wallet: &str) -> Result<bool> {
        // Get webhook details from Helius
        let monitoring = self.db.get_wallet_monitoring(wallet)
            .await?
            .context("Wallet monitoring not found")?;

        let webhook_id = monitoring
            .helius_webhook_id
            .context("No webhook ID found for wallet")?;

        // Get webhook from Helius
        let webhook = self.helius_client.get_webhook(&webhook_id).await?;

        // Check if webhook is active and URL matches
        let is_active = webhook
            .get("isActive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let webhook_url = webhook.get("webhookURL").and_then(|v| v.as_str());

        Ok(is_active && webhook_url == Some(self.config.webhook_url.as_str()))
    }

    /// Reconcile with Helius dashboard using profitability assessment
    ///
    /// This function discovers all webhooks in the Helius dashboard and assesses
    /// the profitability of each wallet to determine which webhooks to keep or delete.
    /// A webhook is kept if ANY of its wallet addresses is eligible.
    /// A webhook is deleted if ALL of its wallet addresses are ineligible.
    pub async fn reconcile_with_helius_dashboard(&self) -> Result<WebhookReconciliationResult> {
        let start = Instant::now();
        info!("Starting Helius dashboard reconciliation with profitability assessment");

        // Fetch all webhooks from Helius
        let helius_webhooks = self
            .helius_client
            .list_webhooks_typed()
            .await
            .context("Failed to list Helius webhooks")?;

        let total_webhooks = helius_webhooks.len();
        let mut eligible_wallets = 0;
        let mut ineligible_wallets = 0;
        let mut deleted_webhooks = 0;
        let mut failed_deletions = 0;
        let mut details = Vec::new();

        info!(total = total_webhooks, "Processing Helius webhooks for profitability");

        for webhook in helius_webhooks {
            let webhook_id = webhook.webhook_id.clone();

            // Check eligibility for each wallet address in the webhook
            let mut any_eligible = false;
            let mut wallet_details = Vec::new();

            for wallet_address in &webhook.wallet_addresses {
                match self.check_wallet_eligibility(wallet_address).await {
                    Ok(eligibility) => {
                        if eligibility.eligible {
                            any_eligible = true;
                            eligible_wallets += 1;
                            info!(
                                webhook_id = %webhook_id,
                                wallet = %wallet_address,
                                wqs = ?eligibility.wqs_score,
                                confidence = ?eligibility.confidence,
                                archetype = %eligibility.archetype,
                                "Wallet eligible for webhook"
                            );
                        } else {
                            ineligible_wallets += 1;
                            warn!(
                                webhook_id = %webhook_id,
                                wallet = %wallet_address,
                                reason = %eligibility.reason,
                                "Wallet ineligible for webhook"
                            );
                        }

                        wallet_details.push(WebhookReconciliationDetail {
                            webhook_id: webhook_id.clone(),
                            wallet_address: wallet_address.clone(),
                            kept: eligibility.eligible,
                            reason: eligibility.reason.clone(),
                        });
                    }
                    Err(e) => {
                        ineligible_wallets += 1;
                        warn!(
                            webhook_id = %webhook_id,
                            wallet = %wallet_address,
                            error = %e,
                            "Failed to check wallet eligibility"
                        );
                        wallet_details.push(WebhookReconciliationDetail {
                            webhook_id: webhook_id.clone(),
                            wallet_address: wallet_address.clone(),
                            kept: false,
                            reason: format!("Eligibility check failed: {}", e),
                        });
                    }
                }
            }

            // Delete webhook if NO wallets are eligible
            if !any_eligible && !webhook.wallet_addresses.is_empty() {
                info!(
                    webhook_id = %webhook_id,
                    wallet_count = webhook.wallet_addresses.len(),
                    "Deleting webhook - no eligible wallets"
                );

                // Rate limit before deletion
                self.rate_limiter
                    .acquire_standard(RequestPriority::Polling)
                    .await;

                match self.helius_client.delete_webhook(&webhook_id).await {
                    Ok(()) => {
                        deleted_webhooks += 1;
                        info!(webhook_id = %webhook_id, "Successfully deleted webhook");
                    }
                    Err(e) => {
                        failed_deletions += 1;
                        error!(webhook_id = %webhook_id, error = %e, "Failed to delete webhook");
                    }
                }
            } else if any_eligible {
                info!(
                    webhook_id = %webhook_id,
                    eligible_count = wallet_details.iter().filter(|d| d.kept).count(),
                    "Keeping webhook - has eligible wallets"
                );
            }

            details.extend(wallet_details);
        }

        let duration_ms = start.elapsed().as_millis() as u64;

        info!(
            total = total_webhooks,
            eligible_wallets,
            ineligible_wallets,
            deleted_webhooks,
            failed_deletions,
            duration_ms,
            "Helius dashboard reconciliation completed"
        );

        Ok(WebhookReconciliationResult {
            total_helius_webhooks: total_webhooks,
            eligible_wallets,
            ineligible_wallets,
            deleted_webhooks,
            failed_deletions,
            duration_ms,
            details,
        })
    }

    /// Check wallet eligibility for webhook based on profitability criteria
    async fn check_wallet_eligibility(&self, wallet_address: &str) -> Result<WebhookEligibility> {
        match self.db.get_wallet(wallet_address).await {
            Ok(Some(wallet)) => {
                use rust_decimal::prelude::*;
                let archetype = wallet.archetype.as_deref().unwrap_or("UNKNOWN");
                let trade_count = wallet.trade_count_30d.unwrap_or(0) as i64;
                let confidence = if trade_count >= 20 {
                    rust_decimal::Decimal::ONE
                } else {
                    rust_decimal::Decimal::from_f64_retain(trade_count as f64 / 20.0).unwrap_or(rust_decimal::Decimal::ZERO)
                };
                let threshold = match archetype {
                    "WHALE" => 55.0,
                    "SWING" => 58.0,
                    _ => 65.0,
                };

                if wallet.status != "ACTIVE" {
                    return Ok(WebhookEligibility {
                        eligible: false,
                        wqs_score: wallet.wqs_score,
                        confidence,
                        status: wallet.status.clone(),
                        archetype: archetype.to_string(),
                        trade_count,
                        roi_7d: wallet.roi_7d,
                        roi_30d: wallet.roi_30d,
                        reason: format!("Wallet status is {} (not ACTIVE)", wallet.status),
                    });
                }

                if let Some(wqs) = wallet.wqs_score {
                    let wqs_f64 = wqs.to_f64().unwrap_or(0.0);
                    if wqs_f64 < threshold {
                        return Ok(WebhookEligibility {
                            eligible: false,
                            wqs_score: wallet.wqs_score,
                            confidence,
                            status: wallet.status,
                            archetype: archetype.to_string(),
                            trade_count,
                            roi_7d: wallet.roi_7d,
                            roi_30d: wallet.roi_30d,
                            reason: format!("WQS {:.1} below threshold {:.1} for archetype {}", wqs_f64, threshold, archetype),
                        });
                    }
                } else {
                    return Ok(WebhookEligibility {
                        eligible: false,
                        wqs_score: None,
                        confidence,
                        status: wallet.status,
                        archetype: archetype.to_string(),
                        trade_count,
                        roi_7d: wallet.roi_7d,
                        roi_30d: wallet.roi_30d,
                        reason: "WQS score is NULL".to_string(),
                    });
                }

                if confidence < rust_decimal::Decimal::from_f64_retain(0.70).unwrap_or(rust_decimal::Decimal::ZERO) {
                    return Ok(WebhookEligibility {
                        eligible: false,
                        wqs_score: wallet.wqs_score,
                        confidence,
                        status: wallet.status,
                        archetype: archetype.to_string(),
                        trade_count,
                        roi_7d: wallet.roi_7d,
                        roi_30d: wallet.roi_30d,
                        reason: format!("Confidence {:.2} below minimum 0.70", confidence.to_f64().unwrap_or(0.0)),
                    });
                }

                if trade_count < 5 {
                    return Ok(WebhookEligibility {
                        eligible: false,
                        wqs_score: wallet.wqs_score,
                        confidence,
                        status: wallet.status,
                        archetype: archetype.to_string(),
                        trade_count,
                        roi_7d: wallet.roi_7d,
                        roi_30d: wallet.roi_30d,
                        reason: format!("Insufficient trades ({} < 5)", trade_count),
                    });
                }

                Ok(WebhookEligibility {
                    eligible: true,
                    wqs_score: wallet.wqs_score,
                    confidence,
                    status: wallet.status,
                    archetype: archetype.to_string(),
                    trade_count,
                    roi_7d: wallet.roi_7d,
                    roi_30d: wallet.roi_30d,
                    reason: format!(
                        "Eligible: WQS {:.1}, confidence {:.2}, {} trades, archetype {}",
                        wallet.wqs_score.map(|v| v.to_f64().unwrap_or(0.0)).unwrap_or(0.0),
                        confidence.to_f64().unwrap_or(0.0),
                        trade_count,
                        archetype
                    ),
                })
            }
            Ok(None) => Ok(WebhookEligibility {
                eligible: false,
                wqs_score: None,
                confidence: rust_decimal::Decimal::ZERO,
                status: "NOT_FOUND".to_string(),
                archetype: "UNKNOWN".to_string(),
                trade_count: 0,
                roi_7d: None,
                roi_30d: None,
                reason: "Wallet not found in database".to_string(),
            }),
            Err(e) => Err(e.into()),
        }
    }

    /// Validate Solana address format
    fn is_valid_solana_address(&self, address: &str) -> bool {
        is_valid_solana_address(address)
    }
}

/// Validate Solana address format
pub fn is_valid_solana_address(address: &str) -> bool {
    // Basic Solana address validation (Base58, 32-44 characters)
    address.len() >= 32
        && address.len() <= 44
        && address.chars().all(|c| {
            c.is_alphanumeric()
                || ['1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F',
                    'G', 'H', 'J', 'K', 'L', 'M', 'N', 'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X',
                    'Y', 'Z', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'm', 'n', 'o', 'p',
                    'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z']
                    .contains(&c)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_solana_address() {
        // Valid addresses
        assert!(is_valid_solana_address("52kpqW23KVMhAC5Lnt5AWRQ63AQtGxrC7paiC5ttf9Tz"));
        assert!(is_valid_solana_address("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"));
        assert!(is_valid_solana_address("So11111111111111111111111111111111111111112"));

        // Invalid addresses
        assert!(!is_valid_solana_address(""));
        assert!(!is_valid_solana_address("too_short"));
        assert!(!is_valid_solana_address("invalid@characters!"));
    }
}
