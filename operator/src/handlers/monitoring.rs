//! Monitoring handlers for automatic copy trading
//!
//! Handles Helius webhook endpoint and monitoring status

use crate::middleware::{AuthExtension, Role};
use crate::models::Action;
use crate::monitoring::transaction_parser::parse_helius_webhook;
use crate::monitoring::HeliusWebhookPayload;
use crate::monitoring::MonitoringState;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Json,
};
use serde::Serialize;
use std::sync::Arc;

/// Helius webhook endpoint
pub async fn helius_webhook_handler(
    State(state): State<Arc<MonitoringState>>,
    headers: HeaderMap,
    Json(payload): Json<Vec<HeliusWebhookPayload>>,
) -> StatusCode {
    // ── Auth header verification (B2, staged) ───────────────────────────
    //
    // When `helius_auth_header` is configured, Helius echoes it in the
    // `Authorization` header of every delivery. In dry-run mode
    // (`helius_auth_enforce=false`) we log the result but always accept;
    // in enforce mode we reject non-matching requests with HTTP 401.
    if let Some(expected) = &state.helius_auth_header {
        let received = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if received == expected {
            tracing::debug!("auth_ok: Helius webhook Authorization header matches");
        } else if state.helius_auth_enforce {
            tracing::warn!(
                received_header = %received,
                "auth_rejected: Helius webhook Authorization header mismatch (enforce mode)"
            );
            return StatusCode::UNAUTHORIZED;
        } else {
            tracing::warn!(
                received_header = %received,
                "auth_mismatch: Helius webhook Authorization header does not match (dry-run, accepting)"
            );
        }
    }
    // Process each event in the array
    for event in payload {
        // Rate limit webhook processing (non-blocking check)
        // Reject events when the limiter is at capacity instead of only
        // tracking usage (the blocking acquire is skipped to avoid Send issues).
        if !state.webhook_rate_limiter.try_acquire() {
            tracing::warn!(
                signature = %event.signature,
                "Webhook rate limit exceeded, skipping event"
            );
            continue;
        }

        // Dedup: skip if this signature was already processed within the last 5 minutes.
        // Multiple orphaned webhooks deliver the same transaction, causing redundant
        // parse/filter cycles that waste CPU and flood logs.
        {
            let mut seen = state.processed_signatures.lock();
            if let Some(ts) = seen.get(&event.signature) {
                if ts.elapsed().as_secs() < 300 {
                    tracing::debug!(
                        signature = %event.signature,
                        "Duplicate webhook event skipped (already processed)"
                    );
                    continue;
                }
            }
            seen.insert(event.signature.clone(), std::time::Instant::now());
            // Periodic cleanup: evict entries older than 10 minutes to bound memory
            if seen.len() > 5000 {
                seen.retain(|_, ts| ts.elapsed().as_secs() < 600);
            }
        }

        tracing::info!(
            signature = %event.signature,
            transaction_type = %event.transaction_type,
            "Received Helius webhook event"
        );

        // ── RPC signature verification (B2, staged) ──────────────────────
        //
        // Fetch the transaction by signature from trusted Solana RPC (via
        // Helius) and confirm it exists. In dry-run mode (`rpc_verify_enforce
        // = false`) we log the result but always accept; in enforce mode we
        // drop events whose signature cannot be confirmed.
        match state.helius_client.verify_signature_exists(&event.signature).await {
            Ok(true) => {
                tracing::debug!(
                    signature = %event.signature,
                    "rpc_verify_ok: transaction confirmed on-chain"
                );
            }
            Ok(false) => {
                if state.rpc_verify_enforce {
                    tracing::warn!(
                        signature = %event.signature,
                        "rpc_verify_rejected: transaction not found on-chain (enforce mode)"
                    );
                    continue;
                } else {
                    tracing::warn!(
                        signature = %event.signature,
                        "rpc_verify_failed: transaction not found on-chain (dry-run, accepting)"
                    );
                }
            }
            Err(e) => {
                if state.rpc_verify_enforce {
                    tracing::warn!(
                        signature = %event.signature,
                        error = %e,
                        "rpc_verify_rejected: RPC fetch failed (enforce mode)"
                    );
                    continue;
                } else {
                    tracing::warn!(
                        signature = %event.signature,
                        error = %e,
                        "rpc_verify_error: RPC fetch failed (dry-run, accepting)"
                    );
                }
            }
        }

        // Resolve tracked wallet address: match userAccount entries against ACTIVE wallets.
        // Uses a 30s TTL cache to avoid a `get_wallets_by_status("ACTIVE")` DB query
        // per webhook event (10K+ events/hour — the dominant DB load before this cache).
        let active_wallet_addresses: std::collections::HashSet<String> = {
            // Cache read scoped to its own block — guard drops before any await.
            let cached_set: Option<std::collections::HashSet<String>> = {
                let cache = state.active_wallet_cache.read();
                match cache.as_ref() {
                    Some((loaded_at, set)) if loaded_at.elapsed().as_secs() < 30 => {
                        Some(set.clone())
                    }
                    _ => None,
                }
            };

            match cached_set {
                Some(set) => set,
                None => {
                    let wallets = match state.db.get_wallets_by_status("ACTIVE").await {
                        Ok(w) => w,
                        Err(e) => {
                            tracing::warn!(error = %e, "Failed to query active wallets, falling back to no filter");
                            vec![]
                        }
                    };
                    let set: std::collections::HashSet<String> =
                        wallets.into_iter().map(|w| w.address).collect();
                    *state.active_wallet_cache.write() =
                        Some((std::time::Instant::now(), set.clone()));
                    set
                }
            }
        };

        let tracked_wallet = {
            let mut matched_wallet: Option<String> = None;
            for account in &event.account_data {
                if let Some(token_changes) = &account.token_balance_changes {
                    for change in token_changes {
                        if active_wallet_addresses.contains(&change.user_account) {
                            matched_wallet = Some(change.user_account.clone());
                            break;
                        }
                    }
                    if matched_wallet.is_some() {
                        break;
                    }
                }
            }

            matched_wallet
        };

        // Parse webhook to extract swap information
        let tracked_wallet_ref = tracked_wallet.as_deref();
        let parsed = parse_helius_webhook(&event, tracked_wallet_ref);
        if let Ok(Some(swap)) = parsed {
            // Only process events that matched an ACTIVE tracked wallet. Guessing
            // a wallet from account_data would create garbage rows in the
            // wallets/speculative-activity tables from unauthenticated webhook data.
            let wallet_address = match tracked_wallet_ref {
                Some(wallet) => wallet.to_string(),
                None => {
                    // This is expected: Helius webhooks fire for many wallets,
                    // most of which are not in our ACTIVE set. Logging at debug
                    // avoids ~13k WARN lines/day of non-actionable noise.
                    tracing::debug!(
                        signature = %event.signature,
                        transaction_type = %event.transaction_type,
                        "Webhook event has no tracked wallet (no ACTIVE wallet matched user_account)"
                    );
                    continue;
                }
            };

            if !wallet_address.is_empty() {
                tracing::info!(
                    wallet = %wallet_address,
                    direction = ?swap.direction,
                    token_out = %swap.token_out,
                    amount_in = %swap.amount_in,
                    tracked_from_db = tracked_wallet.is_some(),
                    "Parsed swap from webhook"
                );
                
                // Record speculative activity for inactivity tracking
                crate::monitoring::record_speculative_activity(state.db.clone(), &wallet_address, &swap.token_out).await;
                
                // Check if wallet exists in database. A DB error must not be
                // treated as "wallet not found" — that would make the upsert
                // below silently overwrite a real wallet's metrics.
                let wallet = match state.db.get_wallet(&wallet_address).await {
                    Ok(Some(w)) => w,
                    Ok(None) => {
                        // Auto-add wallet when detected making a trade
                        tracing::info!(
                            wallet = %wallet_address,
                            "New wallet detected, adding to database"
                        );

                        // Add wallet with minimal info (will be analyzed by Scout later)
                        let _ = state
                            .db
                            .upsert_wallet(
                                &wallet_address,
                                None,                 // wqs_score - will be calculated by Scout
                                None,                 // roi_7d
                                None,                 // roi_30d
                                Some(1),              // trade_count_30d - at least 1 trade detected
                                None,                 // win_rate
                                None,                 // max_drawdown_30d
                                Some(swap.amount_in), // avg_trade_size_sol
                                Some("Auto-added from webhook detection"), // notes
                            )
                            .await;

                        // Fetch the newly added wallet
                        match state.db.get_wallet(&wallet_address).await {
                            Ok(Some(w)) => w,
                            _ => {
                                tracing::warn!(
                                    wallet = %wallet_address,
                                    "Failed to retrieve newly added wallet"
                                );
                                continue; // Skip this event, but continue processing others
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            wallet = %wallet_address,
                            error = %e,
                            "Failed to query wallet, skipping event"
                        );
                        continue;
                    }
                };

                // Only process signals from ACTIVE wallets
                if wallet.status == "ACTIVE" {
                    tracing::debug!(
                        wallet = %wallet_address,
                        "ACTIVE wallet signal accepted for processing"
                    );
                    // FIX 1: Check circuit breaker before queuing
                    if let Some(ref cb) = state.circuit_breaker {
                        if !cb.is_trading_allowed() {
                            let reason = cb
                                .trip_reason()
                                .map(|r| r.to_string())
                                .unwrap_or_else(|| "Circuit breaker tripped".to_string());
                            tracing::warn!(
                                wallet = %wallet_address,
                                reason = %reason,
                                "Helius webhook signal blocked by circuit breaker"
                            );
                            continue; // Skip this event, but continue processing others
                        }
                    }

                    // Generate signal
                    let direction = if swap.direction == crate::monitoring::SwapDirection::Buy {
                        Action::Buy
                    } else {
                        Action::Sell
                    };

                    // B1: Unified decision pipeline — same SelectionService as the
                    // direct webhook path. The copied wallet's swap amount is
                    // telemetry only; PositionSizer governs all BUY sizes.
                    let target_token = if direction == Action::Buy {
                        swap.token_out.clone()
                    } else {
                        swap.token_in.clone()
                    };

                    let selection = match state.selection.as_ref() {
                        Some(s) => s,
                        None => {
                            tracing::error!(
                                wallet = %wallet_address,
                                "SelectionService not configured — cannot process signal"
                            );
                            continue;
                        }
                    };

                    let req = crate::engine::SelectionRequest {
                        wallet_address: wallet_address.clone(),
                        token_address: target_token.clone(),
                        action: direction,
                        source_amount_sol: swap.amount_in,
                        ingress: crate::engine::Ingress::Helius,
                        source_slot: None, // ParsedSwap doesn't carry slot; future: parse from tx
                        exit_fraction: None,
                    };
                    let decision = selection.decide(&req).await;

                    if !decision.admitted {
                        tracing::info!(
                            wallet = %wallet_address,
                            token = %target_token,
                            code = decision.rejection_code.unwrap_or("REJECTED"),
                            reason = decision.rejection_reason.as_deref().unwrap_or("rejected"),
                            "Monitoring signal rejected by selection service"
                        );
                        // Single-wallet unproven BUY or a token without
                        // shadow-mirror history → defer to entry confirmation:
                        // admit only if the whale's entry price holds for the
                        // confirmation window. Replaces "buy the top of a
                        // fresh pump" with "buy only if the whale's entry is
                        // holding" (the losing pattern across all 134
                        // historical closed trades).
                        let confirmable = decision.rejection_code == Some("SINGLE_WALLET_UNPROVEN")
                            || decision.rejection_code == Some("SHADOW_MIRROR_INSUFFICIENT");
                        if confirmable && direction == Action::Buy {
                            if let Some(ref ec) = state.entry_confirmation {
                                let sol_mint = crate::constants::mints::SOL;
                                // Only SOL-quoted buys give a free exact entry
                                // price (amount_in is SOL, amount_out raw units).
                                if swap.token_in == sol_mint
                                    && swap.amount_out > rust_decimal::Decimal::ZERO
                                {
                                    let ref_price = swap.amount_in / swap.amount_out;
                                    if ec.register(req.clone(), ref_price).await {
                                        tracing::info!(
                                            wallet = %wallet_address,
                                            token = %target_token,
                                            ref_price_sol_per_raw = %ref_price,
                                            "Signal queued for entry confirmation (single-wallet unproven)"
                                        );
                                        continue;
                                    }
                                }
                            }
                        }
                        continue;
                    }

                    // Queue the admitted signal through the shared path (also
                    // used by the entry-confirmation loop) — identical UUID,
                    // payload, decimals, insert, link, queue, and status
                    // updates as before.
                    if !crate::engine::entry_confirmation::queue_monitoring_signal(
                        &state.db,
                        &state.engine,
                        state.token_parser.as_ref(),
                        selection,
                        &decision,
                        &req,
                    )
                    .await
                    {
                        continue;
                    }
                } else {
                    tracing::debug!(
                        wallet = %wallet_address,
                        status = %wallet.status,
                        "Wallet detected but not ACTIVE, skipping signal"
                    );
                }
            } else {
                tracing::debug!(
                    signature = %event.signature,
                    "Webhook swap skipped: wallet address is empty"
                );
            }
        } else {
            // Log if we had a tracked wallet but still failed to parse
            if let Some(ref wallet) = tracked_wallet {
                tracing::debug!(
                    signature = %event.signature,
                    tracked_wallet = %wallet,
                    "Webhook event parsed to no swap (Ok(None)) despite tracked wallet"
                );
            }

            // Diagnose why parse returned None/Err so silent signal drops are visible.
            let account_count = event.account_data.len();
            let token_change_count: usize = event
                .account_data
                .iter()
                .map(|a| a.token_balance_changes.as_ref().map(|c| c.len()).unwrap_or(0))
                .sum();
            let native_transfer_count = event.native_transfers.len();
            match parsed {
                Ok(None) => tracing::debug!(
                    signature = %event.signature,
                    transaction_type = %event.transaction_type,
                    account_count,
                    token_change_count,
                    native_transfer_count,
                    "Webhook event parsed to no swap (Ok(None)) — likely no significant non-SOL token delta"
                ),
                Err(e) => tracing::warn!(
                    signature = %event.signature,
                    error = %e,
                    "Webhook event failed to parse"
                ),
                _ => {}
            }
        }
    }

    StatusCode::OK
}

/// Get monitoring status
/// Requires: readonly+ role (matches get_wallet_monitoring_states)
pub async fn get_monitoring_status(
    State(state): State<Arc<MonitoringState>>,
    axum::Extension(auth): axum::Extension<AuthExtension>,
) -> Json<MonitoringStatus> {
    if !auth.0.role.has_permission(Role::Readonly) {
        tracing::warn!("Unauthorized attempt to access monitoring status");
        return Json(MonitoringStatus {
            enabled: false,
            webhook_rate: 0.0,
            rpc_rate: 0.0,
            webhook_credits: 0,
            rpc_credits: 0,
            active_wallets: 0,
        });
    }
    let webhook_rate = state.webhook_rate_limiter.current_rate();
    let rpc_rate = state.rpc_rate_limiter.current_rate();
    let webhook_credits = state.webhook_rate_limiter.credit_usage();
    let rpc_credits = state.rpc_rate_limiter.credit_usage();

    Json(MonitoringStatus {
        enabled: state
            .config
            .monitoring
            .as_ref()
            .map(|m| m.enabled)
            .unwrap_or(false),
        webhook_rate,
        rpc_rate,
        webhook_credits,
        rpc_credits,
        active_wallets: {
            // Query active wallets count from database
            match state.db.get_all_wallet_monitoring().await {
                Ok(records) => records.iter().filter(|r| r.monitoring_enabled).count(),
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to query active wallets count, returning 0");
                    0
                }
            }
        },
    })
}

#[derive(Debug, Serialize)]
pub struct MonitoringStatus {
    enabled: bool,
    webhook_rate: f64,
    rpc_rate: f64,
    webhook_credits: u64,
    rpc_credits: u64,
    active_wallets: usize,
}

/// Enable monitoring for a wallet
/// Requires: operator+ role
pub async fn enable_wallet_monitoring(
    State(state): State<Arc<MonitoringState>>,
    axum::Extension(auth): axum::Extension<AuthExtension>,
    Path(wallet_address): Path<String>,
) -> StatusCode {
    if !auth.0.role.has_permission(Role::Operator) {
        return StatusCode::FORBIDDEN;
    }
    tracing::info!(wallet = %wallet_address, "Enable monitoring requested");

    // Check if wallet exists and is ACTIVE
    let wallet = match state.db.get_wallet(&wallet_address).await {
        Ok(Some(w)) => w,
        Ok(None) => {
            tracing::warn!(wallet = %wallet_address, "Wallet not found");
            return StatusCode::NOT_FOUND;
        }
        Err(e) => {
            tracing::error!(wallet = %wallet_address, error = %e, "Failed to query wallet");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    if wallet.status != "ACTIVE" {
        tracing::warn!(
            wallet = %wallet_address,
            status = %wallet.status,
            "Wallet is not ACTIVE, cannot enable monitoring"
        );
        return StatusCode::BAD_REQUEST;
    }

    // Short-circuit if monitoring is already enabled: calling enable twice must
    // not register a second (orphaned) Helius webhook.
    if let Ok(Some(existing)) = state.db.get_wallet_monitoring(&wallet_address).await {
        if existing.monitoring_enabled {
            if let Some(webhook_id) = &existing.helius_webhook_id {
                if !webhook_id.is_empty() {
                    tracing::info!(
                        wallet = %wallet_address,
                        webhook_id = %webhook_id,
                        "Wallet monitoring already enabled, reusing existing webhook"
                    );
                    return StatusCode::OK;
                }
            }
        }
    }

    // Get webhook URL from config
    let webhook_url = match &state.config.monitoring {
        Some(m) => m.helius_webhook_url.as_ref(),
        None => {
            tracing::error!("Monitoring config not available");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    let webhook_url = match webhook_url {
        Some(url) => url,
        None => {
            tracing::error!("Helius webhook URL not configured");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    // Register Helius webhook for this wallet
    let wallets = vec![wallet_address.clone()];
    let resolved_auth_header = state
        .config
        .monitoring
        .as_ref()
        .and_then(|m| m.resolved_helius_auth_header());
    let webhook_id = match state
        .helius_client
        .register_webhook(
            &wallets,
            webhook_url,
            resolved_auth_header.as_deref(),
        )
        .await
    {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(
                wallet = %wallet_address,
                error = %e,
                "Failed to register Helius webhook"
            );
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    // Update database
    if let Err(e) = state
        .db
        .upsert_wallet_monitoring(&wallet_address, Some(&webhook_id), true)
        .await
    {
        tracing::error!(
            wallet = %wallet_address,
            error = %e,
            "Failed to update wallet_monitoring in database"
        );
        // Try to clean up webhook registration
        let _ = state.helius_client.delete_webhook(&webhook_id).await;
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    tracing::info!(
        wallet = %wallet_address,
        webhook_id = %webhook_id,
        "Wallet monitoring enabled successfully"
    );

    StatusCode::OK
}

/// Disable monitoring for a wallet
/// Requires: operator+ role
pub async fn disable_wallet_monitoring(
    State(state): State<Arc<MonitoringState>>,
    axum::Extension(auth): axum::Extension<AuthExtension>,
    Path(wallet_address): Path<String>,
) -> StatusCode {
    if !auth.0.role.has_permission(Role::Operator) {
        return StatusCode::FORBIDDEN;
    }
    tracing::info!(wallet = %wallet_address, "Disable monitoring requested");

    // Get current monitoring record
    let monitoring = match state.db.get_wallet_monitoring(&wallet_address).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            tracing::warn!(wallet = %wallet_address, "Wallet monitoring not found");
            return StatusCode::NOT_FOUND;
        }
        Err(e) => {
            tracing::error!(wallet = %wallet_address, error = %e, "Failed to query wallet monitoring");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    // Delete Helius webhook if it exists. If deletion fails, the webhook stays
    // live and keeps delivering events, so report the failure instead of
    // marking monitoring disabled while events keep being processed.
    if let Some(webhook_id) = &monitoring.helius_webhook_id {
        if let Err(e) = state.helius_client.delete_webhook(webhook_id).await {
            tracing::error!(
                wallet = %wallet_address,
                webhook_id = %webhook_id,
                error = %e,
                "Failed to delete Helius webhook, monitoring remains enabled"
            );
            return StatusCode::INTERNAL_SERVER_ERROR;
        } else {
            tracing::info!(
                wallet = %wallet_address,
                webhook_id = %webhook_id,
                "Helius webhook deleted successfully"
            );
        }
    }

    // Update database to disable monitoring
    if let Err(e) = state
        .db
        .upsert_wallet_monitoring(
            &wallet_address,
            None,  // Clear webhook_id
            false, // Disable monitoring
        )
        .await
    {
        tracing::error!(
            wallet = %wallet_address,
            error = %e,
            "Failed to update wallet_monitoring in database"
        );
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    tracing::info!(
        wallet = %wallet_address,
        "Wallet monitoring disabled successfully"
    );

    StatusCode::OK
}

/// Wallet monitoring state response
#[derive(Debug, Serialize)]
pub struct WalletMonitoringStateResponse {
    pub wallet_states: Vec<WalletMonitoringStateItem>,
}

#[derive(Debug, Serialize)]
pub struct WalletMonitoringStateItem {
    pub address: String,
    pub method: String, // "webhook" or "polling"
    pub status: String, // "active", "inactive", or "error"
    pub last_activity: String,
    pub last_fetch: Option<String>,
    pub failed_fetches: i32,
    pub success_rate: f64,
    pub next_fetch: Option<String>,
}

/// Get all wallet monitoring states
/// Requires: readonly+ role
pub async fn get_wallet_monitoring_states(
    State(state): State<Arc<MonitoringState>>,
    axum::Extension(auth): axum::Extension<AuthExtension>,
) -> Json<WalletMonitoringStateResponse> {
    // Verify user has at least readonly access
    if !auth.0.role.has_permission(Role::Readonly) {
        tracing::warn!("Unauthorized attempt to access wallet monitoring states");
        return Json(WalletMonitoringStateResponse {
            wallet_states: vec![],
        });
    }

    // Fetch all wallet monitoring records from database
    let wallet_monitoring_records = match state.db.get_all_wallet_monitoring().await {
        Ok(records) => records,
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch wallet monitoring states");
            return Json(WalletMonitoringStateResponse {
                wallet_states: vec![],
            });
        }
    };

    // Transform database records to frontend format
    let wallet_states: Vec<WalletMonitoringStateItem> = wallet_monitoring_records
        .into_iter()
        .map(|wm| {
            // Determine method: webhook if helius_webhook_id exists, otherwise polling
            let method = if wm.helius_webhook_id.is_some()
                && !wm.helius_webhook_id.as_ref().unwrap().is_empty()
            {
                "webhook".to_string()
            } else {
                "polling".to_string()
            };

            // Determine status based on monitoring_enabled and webhook_health_status
            let status = if !wm.monitoring_enabled {
                "inactive".to_string()
            } else if wm.webhook_health_status.as_deref() == Some("error")
                || wm.webhook_health_status.as_deref() == Some("unhealthy")
                || wm.webhook_status.as_deref() == Some("failed")
            {
                "error".to_string()
            } else {
                "active".to_string()
            };

            // Calculate success rate based on registration attempts
            // If no attempts, assume 100%, otherwise calculate based on failures
            let success_rate = if wm.registration_attempts == 0 {
                100.0
            } else {
                let base_rate = 100.0;
                // Penalize for failed registration attempts
                let failure_penalty =
                    (wm.last_registration_error.as_ref().is_some() as i32 as f64) * 10.0;
                (base_rate - failure_penalty).max(0.0)
            };

            // Use registration_attempts as failed_fetches indicator
            let failed_fetches = wm.registration_attempts;

            // Set last_activity from last_monitored_at, fallback to created_at
            let last_activity = wm
                .last_monitored_at
                .clone()
                .or(Some(wm.created_at))
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());

            // Set last_fetch to last_monitored_at if available
            let last_fetch = wm.last_monitored_at.clone();

            // Calculate next_fetch: for webhooks it's null (real-time),
            // for polling we'll estimate 15 minutes from last activity
            let next_fetch = if method == "polling" {
                Some(
                    chrono::Utc::now()
                        .checked_add_signed(chrono::Duration::minutes(15))
                        .unwrap_or_else(|| chrono::Utc::now() + chrono::Duration::minutes(15))
                        .to_rfc3339(),
                )
            } else {
                None
            };

            WalletMonitoringStateItem {
                address: wm.wallet_address,
                method,
                status,
                last_activity,
                last_fetch,
                failed_fetches,
                success_rate,
                next_fetch,
            }
        })
        .collect();

    tracing::info!(
        count = wallet_states.len(),
        "Fetched wallet monitoring states"
    );

    Json(WalletMonitoringStateResponse { wallet_states })
}
