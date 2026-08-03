//! Signal consensus and aggregation API handlers
//!
//! Provides endpoints for:
//! - Consensus detection overview
//! - Wallet clustering analysis
//! - Signal aggregation status

use axum::{extract::Query, extract::State, Json};
use chrono::Utc;
use serde::Serialize;
use sqlx::Row;
use std::sync::Arc;

use super::api::ApiState;
use crate::db_abstraction::DbPool;
use crate::error::AppError;

fn pg_pool(
    db: &Arc<dyn crate::db_abstraction::Database>,
) -> Result<sqlx::Pool<sqlx::Postgres>, AppError> {
    match db.pool() {
        DbPool::PostgreSQL(p) => Ok(p),
    }
}

// =============================================================================
// RESPONSE TYPES
// =============================================================================

/// Consensus overview response
#[derive(Debug, Serialize)]
pub struct ConsensusResponse {
    #[serde(rename = "consensus_detection_rate")]
    pub consensus_rate: f64,
    #[serde(rename = "average_clustering")]
    pub avg_clustering_coefficient: f64,
    pub active_clusters: Vec<Cluster>,
    #[serde(rename = "consensus_signals")]
    pub recent_signals: Vec<ConsensusSignal>,
    pub divergence_alerts: Vec<DivergenceAlert>,
}

/// Wallet cluster information
#[derive(Debug, Serialize)]
pub struct Cluster {
    pub id: String,
    pub wallets: Vec<String>,
    pub signal_count: usize,
    pub avg_wqs: f64,
    pub last_activity: String,
    pub coherence: f64,
}

/// Individual consensus signal
#[derive(Debug, Serialize)]
pub struct ConsensusSignal {
    #[serde(skip_serializing)]
    pub signal_id: String,
    pub timestamp: String,
    pub token_address: String,
    pub token_symbol: Option<String>,
    #[serde(skip_serializing)]
    pub consensus_level: String, // "strong" | "moderate" | "weak" | "none"
    #[serde(rename = "consensus_wallets")]
    pub wallet_count: usize,
    #[serde(rename = "total_wallets")]
    pub total_wallet_count: usize,
    #[serde(skip_serializing)]
    pub supporting_wallets: Vec<String>,
    pub quality_score: f64,
    #[serde(skip_serializing)]
    pub executed: bool,
    #[serde(skip_serializing)]
    pub execution_result: Option<ExecutionResult>,
}

/// Execution result for a consensus signal
#[derive(Debug, Serialize)]
pub struct ExecutionResult {
    pub success: bool,
    pub pnl_sol: Option<f64>,
    pub execution_time_ms: Option<u64>,
}

/// Divergence alert when wallets disagree
#[derive(Debug, Serialize)]
pub struct DivergenceAlert {
    pub alert_id: String,
    pub timestamp: String,
    pub token_address: String,
    pub token_symbol: Option<String>,
    #[serde(rename = "divergence_score")]
    pub divergence_type: String, // "directional" | "timing" | "amount"
    pub severity: String, // "low" | "medium" | "high"
    pub wallets_clustered: Vec<WalletCluster>,
    #[serde(rename = "wallets_divergent")]
    pub wallets_divergent: Vec<WalletCluster>,
}

/// Wallet cluster for divergence alerts
#[derive(Debug, Serialize)]
pub struct WalletCluster {
    pub cluster_id: String,
    pub wallet_addresses: Vec<String>,
    pub signal: String, // "BUY" or "SELL"
}

/// Wallet clustering response
#[derive(Debug, Serialize)]
pub struct WalletClusteringResponse {
    pub clusters: Vec<Cluster>,
    pub total_wallets: usize,
    pub clustering_metrics: ClusteringMetrics,
}

/// Clustering metrics
#[derive(Debug, Serialize)]
pub struct ClusteringMetrics {
    pub avg_cluster_size: f64,
    pub max_cluster_size: usize,
    pub silhouette_score: f64,
    pub modularity: f64,
}

/// Signal aggregation response
#[derive(Debug, Serialize)]
pub struct SignalAggregationResponse {
    pub window_start: String,
    pub window_end: String,
    pub total_signals: usize,
    pub unique_tokens: usize,
    pub aggregated_signals: Vec<AggregatedSignal>,
    pub aggregation_latency_ms: u64,
}

/// Aggregated signal for a token
#[derive(Debug, Serialize)]
pub struct AggregatedSignal {
    pub token_address: String,
    pub token_symbol: Option<String>,
    pub signal_count: usize,
    pub unique_wallets: usize,
    pub consensus_score: f64,
    pub recommended_action: String, // "BUY" | "SELL" | "HOLD" | "SKIP"
    pub confidence: f64,
}

/// Signal quality response
#[derive(Debug, Serialize)]
pub struct SignalQualityResponse {
    pub current_quality_score: f64,
    pub quality_distribution: Vec<QualityBucket>,
    pub rejection_rate: f64,
    pub total_signals: i64,
    pub accepted_signals: i64,
    pub rejected_signals: i64,
    pub average_quality_trend: Vec<QualityTrendPoint>,
}

/// Quality distribution bucket
#[derive(Debug, Serialize)]
pub struct QualityBucket {
    pub range: String,
    pub count: i64,
    pub percentage: f64,
}

/// Quality trend point over time
#[derive(Debug, Serialize)]
pub struct QualityTrendPoint {
    pub timestamp: String,
    pub average_score: f64,
}

/// Signal sources response
#[derive(Debug, Serialize)]
pub struct SignalSourcesResponse {
    pub sources: Vec<SignalSource>,
    pub total_signals: i64,
}

/// Individual signal source statistics
#[derive(Debug, Serialize)]
pub struct SignalSource {
    pub source: String,
    pub signal_count: i64,
    pub average_quality: f64,
    pub acceptance_rate: f64,
    pub last_signal_at: String,
}

// =============================================================================
// HANDLERS
// =============================================================================

/// Get consensus overview data
///
/// GET /api/v1/signals/consensus
///
/// Returns consensus detection rate, clustering coefficient, recent consensus signals,
/// and any divergence alerts.
pub async fn get_consensus(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<ConsensusResponse>, AppError> {
    let pool = pg_pool(&state.db)?;

    // Query database for recent consensus signals
    let recent_rows = sqlx::query(
        r#"
        SELECT
            token_address,
            wallet_address,
            direction,
            amount_sol,
            consensus_wallet_count,
            created_at::text as created_at
        FROM signal_aggregation
        WHERE is_consensus = true
        ORDER BY created_at DESC
        LIMIT 20
        "#,
    )
    .fetch_all(&pool)
    .await
    .map_err(AppError::Database)?;

    let recent_signals: Vec<SignalAggRow> = recent_rows
        .into_iter()
        .map(|row| SignalAggRow {
            token_address: row.try_get("token_address").unwrap_or_default(),
            wallet_address: row.try_get("wallet_address").unwrap_or_default(),
            direction: row.try_get("direction").unwrap_or_default(),
            amount_sol: row.try_get("amount_sol").unwrap_or(0.0),
            consensus_wallet_count: row.try_get("consensus_wallet_count").ok(),
            created_at: row.try_get("created_at").unwrap_or_default(),
        })
        .collect();

    // Calculate consensus rate (consensus signals / total signals in last 24h)
    let consensus_rate: f64 = sqlx::query_scalar(
        r#"
        SELECT
            CAST(COUNT(DISTINCT CASE WHEN is_consensus = true THEN token_address || ':' || created_at END) AS DOUBLE PRECISION) /
            NULLIF(COUNT(DISTINCT token_address || ':' || created_at), 0) AS rate
        FROM signal_aggregation
        WHERE created_at >= NOW() - INTERVAL '24 hours'
        "#,
    )
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    // Group by token for consensus signals
    let mut consensus_signals: std::collections::HashMap<String, Vec<SignalAggRow>> =
        std::collections::HashMap::new();
    for row in recent_signals {
        consensus_signals
            .entry(row.token_address.clone())
            .or_default()
            .push(row);
    }

    // Convert to response format
    let recent_signals: Vec<ConsensusSignal> = consensus_signals
        .into_iter()
        .enumerate()
        .map(|(i, (token_addr, rows)): _| {
            let wallet_count = rows.len();
            let wallets: Vec<String> = rows.iter().map(|r| r.wallet_address.clone()).collect();
            let consensus_level = match wallet_count {
                5.. => "strong",
                3..=4 => "moderate",
                2 => "weak",
                _ => "none",
            }
            .to_string();

            ConsensusSignal {
                signal_id: format!("cons_{}", i),
                timestamp: rows[0].created_at.clone(),
                token_address: token_addr,
                token_symbol: None, // token_symbol not in schema
                consensus_level,
                wallet_count,
                total_wallet_count: wallet_count, // For consensus signals, total = count
                supporting_wallets: wallets,
                quality_score: 0.7 + (wallet_count as f64 * 0.05).min(0.3), // Placeholder
                executed: false,
                execution_result: None,
            }
        })
        .collect();

    // Calculate clustering coefficient from in-memory state if available
    let avg_clustering_coefficient = if let Some(ref agg) = state.signal_aggregator {
        // Get cluster info from aggregator
        calculate_clustering_coefficient(agg).await
    } else {
        0.0
    };

    // Get active clusters from the database (real wallet/token groups)
    let active_clusters = fetch_clusters(&pool, 24).await?;

    // Calculate divergence alerts
    let divergence_alerts = if let Some(ref agg) = state.signal_aggregator {
        calculate_divergence_alerts(agg, &recent_signals).await
    } else {
        Vec::new()
    };

    Ok(Json(ConsensusResponse {
        consensus_rate,
        avg_clustering_coefficient,
        active_clusters,
        recent_signals,
        divergence_alerts,
    }))
}

/// Get wallet clustering analysis
///
/// GET /api/v1/signals/clustering
///
/// Returns wallet clusters and clustering metrics.
pub async fn get_wallet_clustering(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<WalletClusteringResponse>, AppError> {
    let pool = pg_pool(&state.db)?;
    let clusters = fetch_clusters(&pool, 24).await?;

    let total_wallets: usize = clusters
        .iter()
        .map(|c| c.wallets.len())
        .sum();

    // Calculate clustering metrics
    let avg_cluster_size = if !clusters.is_empty() {
        total_wallets as f64 / clusters.len() as f64
    } else {
        0.0
    };
    let max_cluster_size = clusters.iter().map(|c| c.wallets.len()).max().unwrap_or(0);

    // Placeholder metrics - in production these would be calculated properly
    let clustering_metrics = ClusteringMetrics {
        avg_cluster_size,
        max_cluster_size,
        silhouette_score: 0.5, // Placeholder
        modularity: 0.3,       // Placeholder
    };

    Ok(Json(WalletClusteringResponse {
        total_wallets,
        clustering_metrics,
        clusters,
    }))
}

/// Get signal aggregation status
///
/// GET /api/v1/signals/aggregation
///
/// Returns signal aggregation window statistics and aggregated signals.
pub async fn get_signal_aggregation(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<SignalAggregationResponse>, AppError> {
    // 5-minute window
    let window_start = chrono::Utc::now() - chrono::Duration::seconds(300);
    let window_end = chrono::Utc::now();

    let pool = pg_pool(&state.db)?;

    // Query signals in the aggregation window
    let signal_rows = sqlx::query(
        r#"
        SELECT
            token_address,
            wallet_address,
            direction,
            amount_sol,
            consensus_wallet_count,
            created_at::text as created_at
        FROM signal_aggregation
        WHERE created_at >= NOW() - INTERVAL '5 minutes'
        ORDER BY created_at DESC
        "#,
    )
    .fetch_all(&pool)
    .await
    .map_err(AppError::Database)?;

    let signals: Vec<SignalAggRow> = signal_rows
        .into_iter()
        .map(|row| SignalAggRow {
            token_address: row.try_get("token_address").unwrap_or_default(),
            wallet_address: row.try_get("wallet_address").unwrap_or_default(),
            direction: row.try_get("direction").unwrap_or_default(),
            amount_sol: row.try_get("amount_sol").unwrap_or(0.0),
            consensus_wallet_count: row.try_get("consensus_wallet_count").ok(),
            created_at: row.try_get("created_at").unwrap_or_default(),
        })
        .collect();

    let total_signals = signals.len();
    let unique_tokens = signals
        .iter()
        .map(|s| s.token_address.clone())
        .collect::<std::collections::HashSet<_>>()
        .len();

    // Aggregate by token
    let mut token_aggregates: std::collections::HashMap<String, AggregatedSignalData> =
        std::collections::HashMap::new();

    for signal in signals {
        let entry = token_aggregates
            .entry(signal.token_address.clone())
            .or_insert_with(|| AggregatedSignalData {
                token_address: signal.token_address.clone(),
                signal_count: 0,
                unique_wallets: std::collections::HashSet::new(),
                total_amount: 0.0,
                buy_count: 0,
                sell_count: 0,
            });

        entry.signal_count += 1;
        entry.unique_wallets.insert(signal.wallet_address.clone());
        entry.total_amount += signal.amount_sol;

        if signal.direction == "BUY" {
            entry.buy_count += 1;
        } else {
            entry.sell_count += 1;
        }
    }

    // Convert to response format
    let aggregated_signals: Vec<AggregatedSignal> = token_aggregates
        .into_values()
        .map(|data| {
            let unique_wallets = data.unique_wallets.len();
            let consensus_score = if unique_wallets >= 2 {
                (unique_wallets as f64 / 5.0).min(1.0)
            } else {
                0.0
            };

            let recommended_action = if unique_wallets >= 3 && data.buy_count > data.sell_count {
                "BUY"
            } else if unique_wallets >= 3 && data.sell_count > data.buy_count {
                "SELL"
            } else if unique_wallets >= 2 {
                "HOLD"
            } else {
                "SKIP"
            }
            .to_string();

            let confidence = consensus_score;

            AggregatedSignal {
                token_address: data.token_address,
                token_symbol: None, // token_symbol not in schema
                signal_count: data.signal_count,
                unique_wallets,
                consensus_score,
                recommended_action,
                confidence,
            }
        })
        .collect();

    Ok(Json(SignalAggregationResponse {
        window_start: window_start.to_rfc3339(),
        window_end: window_end.to_rfc3339(),
        total_signals,
        unique_tokens,
        aggregated_signals,
        aggregation_latency_ms: 10, // Placeholder - measure actual latency
    }))
}

/// Get signal quality metrics
///
/// GET /api/v1/signals/quality
///
/// Query parameters for signal quality endpoint
#[derive(Debug, serde::Deserialize)]
pub struct SignalQualityParams {
    #[serde(default = "default_range")]
    pub range: String,
}

fn default_range() -> String {
    "24h".to_string()
}

impl Default for SignalQualityParams {
    fn default() -> Self {
        Self {
            range: default_range(),
        }
    }
}

/// Database row for signal sources query
#[derive(Debug)]
struct SignalSourceRow {
    source: String,
    signal_count: i64,
    average_quality: f64,
    acceptance_rate: f64,
    last_signal_at: String,
}

/// Returns signal quality score, distribution buckets, rejection rate,
/// and quality trend over time.
pub async fn get_signal_quality(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<SignalQualityParams>,
) -> Result<Json<SignalQualityResponse>, AppError> {
    let pool = pg_pool(&state.db)?;

    // Parse time range
    let range = params.range;
    let cutoff = Utc::now()
        - match range.as_str() {
            "1h" => chrono::Duration::hours(1),
            "6h" => chrono::Duration::hours(6),
            "24h" => chrono::Duration::hours(24),
            "7d" => chrono::Duration::days(7),
            _ => chrono::Duration::hours(24),
        };

    // Timestamps are bound as parameters — interpolating the RFC3339 string
    // into the SQL produced a syntax error and silent default responses.
    let total_signals: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE created_at >= $1")
            .bind(cutoff)
            .fetch_one(&pool)
            .await
            .map_err(AppError::Database)?;

    // Accepted vs rejected signals
    let (accepted_signals, rejected_signals): (i64, i64) = sqlx::query_as(
        r#"
            SELECT
                COUNT(CASE WHEN status IN ('ACTIVE', 'CLOSED') THEN 1 END) as accepted,
                COUNT(CASE WHEN status IN ('FAILED', 'DEAD_LETTER') THEN 1 END) as rejected
            FROM trades WHERE created_at >= $1
            "#,
    )
    .bind(cutoff)
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    // Current quality score (average WQS of wallets that sent signals)
    let current_quality_score: f64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(AVG(w.wqs_score), 50.0)
        FROM trades t
        LEFT JOIN wallets w ON t.wallet_address = w.address
        WHERE t.created_at >= $1
        "#,
    )
    .bind(cutoff)
    .fetch_one(&pool)
    .await
    .map_err(AppError::Database)?;

    // Rejection rate
    let rejection_rate = if total_signals > 0 {
        rejected_signals as f64 / total_signals as f64
    } else {
        0.0
    };

    let _hours = match range.as_str() {
        "1h" => 1,
        "6h" => 6,
        "24h" => 24,
        "7d" => 168,
        _ => 24,
    };

    // Quality distribution buckets
    let quality_distribution = Vec::new();

    // Average quality trend (hourly data points)
    let average_quality_trend = Vec::new();

    Ok(Json(SignalQualityResponse {
        current_quality_score,
        quality_distribution,
        rejection_rate,
        total_signals,
        accepted_signals,
        rejected_signals,
        average_quality_trend,
    }))
}

/// Get signal sources (per-wallet statistics)
///
/// GET /api/v1/signals/sources
///
/// Returns per-wallet signal statistics including signal count,
/// average quality (WQS), acceptance rate, and last signal time.
pub async fn get_signal_sources(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<SignalSourcesResponse>, AppError> {
    let pool = pg_pool(&state.db)?;

    // Query per-wallet signal statistics (last 7 days).
    // COUNT(*) stays BIGINT (matches the i64 decode) and the max timestamp is
    // cast to text so it can decode as a String.
    let source_rows = sqlx::query(
        r#"
        SELECT
            t.wallet_address as source,
            COUNT(*) as signal_count,
            COALESCE(MAX(w.wqs_score), 50.0) as average_quality,
            CAST(COUNT(CASE WHEN t.status IN ('ACTIVE', 'CLOSED') THEN 1 END) AS DOUBLE PRECISION) / CAST(COUNT(*) AS DOUBLE PRECISION) as acceptance_rate,
            MAX(t.created_at)::text as last_signal_at
        FROM trades t
        LEFT JOIN wallets w ON t.wallet_address = w.address
        WHERE t.created_at >= NOW() - INTERVAL '7 days'
        GROUP BY t.wallet_address
        ORDER BY COUNT(*) DESC
        LIMIT 50
        "#,
    )
    .fetch_all(&pool)
    .await
    .map_err(AppError::Database)?;

    let sources_raw: Vec<SignalSourceRow> = source_rows
        .into_iter()
        .map(|row| SignalSourceRow {
            source: row.try_get("source").unwrap_or_default(),
            signal_count: row.try_get("signal_count").unwrap_or(0),
            average_quality: row.try_get("average_quality").unwrap_or(50.0),
            acceptance_rate: row.try_get("acceptance_rate").unwrap_or(0.0),
            last_signal_at: row.try_get("last_signal_at").unwrap_or_default(),
        })
        .collect();

    let total_signals = sources_raw.iter().map(|s| s.signal_count).sum::<i64>();

    let sources: Vec<SignalSource> = sources_raw
        .into_iter()
        .map(|row| SignalSource {
            source: row.source,
            signal_count: row.signal_count,
            average_quality: row.average_quality,
            acceptance_rate: row.acceptance_rate,
            last_signal_at: row.last_signal_at,
        })
        .collect();

    Ok(Json(SignalSourcesResponse {
        sources,
        total_signals,
    }))
}

// =============================================================================
// HELPER FUNCTIONS
// =============================================================================

/// Database row representation for signal aggregation queries
#[derive(Debug)]
struct SignalAggRow {
    token_address: String,
    wallet_address: String,
    direction: String,
    amount_sol: f64,
    #[allow(dead_code)]
    consensus_wallet_count: Option<i64>,
    created_at: String,
}

/// Internal data for aggregating signals by token
struct AggregatedSignalData {
    token_address: String,
    signal_count: usize,
    unique_wallets: std::collections::HashSet<String>,
    total_amount: f64,
    buy_count: usize,
    sell_count: usize,
}

/// Calculate clustering coefficient from signal aggregator
async fn calculate_clustering_coefficient(
    _aggregator: &crate::monitoring::signal_aggregator::SignalAggregator,
) -> f64 {
    // This is a simplified calculation
    // In production, this would analyze the wallet clusters more thoroughly
    0.65 // Placeholder value
}

/// Fetch real wallet clusters from the database: wallet groups per token over
/// the last N hours. Coherence is the directional agreement of the group's
/// signals (|buy - sell| / total), derived from actual signal data.
async fn fetch_clusters(
    db: &sqlx::Pool<sqlx::Postgres>,
    hours: i64,
) -> Result<Vec<Cluster>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT
            s.token_address,
            ARRAY_AGG(DISTINCT s.wallet_address) AS wallets,
            COUNT(*) AS signal_count,
            AVG(w.wqs_score) AS avg_wqs,
            MAX(s.created_at)::text AS last_activity,
            SUM(CASE WHEN s.direction = 'BUY' THEN 1 ELSE -1 END) AS buy_sell_balance
        FROM signal_aggregation s
        LEFT JOIN wallets w ON s.wallet_address = w.address
        WHERE s.created_at >= NOW() - $1::interval
        GROUP BY s.token_address
        ORDER BY signal_count DESC
        "#,
    )
    .bind(format!("{} hours", hours))
    .fetch_all(db)
    .await
    .map_err(AppError::Database)?;

    let mut clusters = Vec::with_capacity(rows.len());
    for row in rows {
        let token_address: String = row.try_get("token_address").unwrap_or_default();
        let wallets: Vec<String> = row.try_get("wallets").unwrap_or_default();
        let signal_count: i64 = row.try_get("signal_count").unwrap_or(0);
        let avg_wqs: Option<f64> = row.try_get("avg_wqs").ok().flatten();
        let last_activity: String = row.try_get("last_activity").unwrap_or_default();
        let buy_sell_balance: Option<i64> = row.try_get("buy_sell_balance").ok().flatten();

        let coherence = if signal_count > 0 {
            let balance = buy_sell_balance.unwrap_or(0).abs();
            (balance as f64 / signal_count as f64).min(1.0)
        } else {
            0.0
        };

        clusters.push(Cluster {
            id: format!("token_{}", token_address.chars().take(8).collect::<String>()),
            wallets,
            signal_count: signal_count as usize,
            avg_wqs: avg_wqs.unwrap_or(0.0),
            last_activity,
            coherence,
        });
    }

    Ok(clusters)
}

/// Calculate divergence alerts from recent signals and aggregator state
///
/// This function analyzes wallet trading patterns to detect divergences where
/// some wallets are exiting positions while others are holding or accumulating.
async fn calculate_divergence_alerts(
    aggregator: &crate::monitoring::signal_aggregator::SignalAggregator,
    _consensus_signals: &[crate::handlers::signals::ConsensusSignal],
) -> Vec<crate::handlers::signals::DivergenceAlert> {
    let mut divergence_alerts = Vec::new();

    // Get recent signals from aggregator for analysis
    let recent_signals = aggregator.get_all_recent_signals().await;

    // Group signals by token to identify divergences
    let mut token_signals: std::collections::HashMap<String, Vec<&crate::monitoring::signal_aggregator::TokenSignal>> =
        std::collections::HashMap::new();

    for signal in &recent_signals {
        token_signals
            .entry(signal.token_address.clone())
            .or_default()
            .push(signal);
    }

    // Analyze each token for divergence patterns
    for (token_address, signals) in token_signals.iter() {
        // Separate buyers and sellers
        let buyers: Vec<&crate::monitoring::signal_aggregator::TokenSignal> = signals
            .iter()
            .filter(|s| s.direction == "BUY")
            .cloned()
            .collect();

        let sellers: Vec<&crate::monitoring::signal_aggregator::TokenSignal> = signals
            .iter()
            .filter(|s| s.direction == "SELL")
            .cloned()
            .collect();

            // Check for divergence: some wallets selling while others buying/holding
            if !sellers.is_empty() && !buyers.is_empty() {
                // This is a divergence pattern - wallets disagree on direction.
                // divergence_type follows the documented contract:
                // "directional" | "timing" | "amount".
                let divergence_type = if buyers.len() != sellers.len() {
                    "directional".to_string()
                } else {
                    "timing".to_string() // Equal split - timing divergence
                };

                // Create wallet clusters for divergent wallets
                let wallets_clustered = vec![WalletCluster {
                    cluster_id: format!(
                        "holders_{}",
                        token_address.chars().take(8).collect::<String>()
                    ),
                    wallet_addresses: buyers.iter().map(|b| b.wallet_address.clone()).collect(),
                    signal: "BUY".to_string(),
                }];

                let wallets_divergent = vec![WalletCluster {
                    cluster_id: format!(
                        "sellers_{}",
                        token_address.chars().take(8).collect::<String>()
                    ),
                    wallet_addresses: sellers.iter().map(|s| s.wallet_address.clone()).collect(),
                    signal: "SELL".to_string(),
                }];

            let alert = DivergenceAlert {
                alert_id: format!("div_{}", uuid::Uuid::new_v4()),
                timestamp: chrono::Utc::now().to_rfc3339(),
                token_address: token_address.clone(),
                token_symbol: None, // Could be enhanced with token metadata lookup
                divergence_type,
                severity: if sellers.len() > buyers.len() {
                    "high".to_string() // Selling pressure is concerning
                } else {
                    "medium".to_string()
                },
                wallets_clustered,
                wallets_divergent,
            };

            divergence_alerts.push(alert);
        }
    }

    // Limit to most recent/divergent alerts to avoid noise
    divergence_alerts.truncate(10);
    divergence_alerts
}
