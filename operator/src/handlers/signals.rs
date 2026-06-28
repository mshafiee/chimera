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

fn sqlite_pool(
    db: &Arc<dyn crate::db_abstraction::Database>,
) -> Result<sqlx::Pool<sqlx::Sqlite>, AppError> {
    match db.pool() {
        DbPool::SQLite(p) => Ok(p),
        _ => Err(AppError::Internal(
            "Only SQLite backend supported".to_string(),
        )),
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
    #[serde(skip_serializing)]
    pub alert_id: String,
    pub timestamp: String,
    pub token_address: String,
    pub token_symbol: Option<String>,
    #[serde(rename = "divergence_score")]
    pub divergence_type: String, // "directional" | "timing" | "amount"
    #[serde(skip_serializing)]
    pub severity: String, // "low" | "medium" | "high"
    #[serde(skip_serializing)]
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
    let pool = sqlite_pool(&state.db)?;

    // Query database for recent consensus signals
    let recent_rows = sqlx::query(
        r#"
        SELECT
            token_address,
            wallet_address,
            direction,
            amount_sol,
            consensus_wallet_count,
            datetime(created_at) as created_at
        FROM signal_aggregation
        WHERE is_consensus = 1
        ORDER BY created_at DESC
        LIMIT 20
        "#,
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

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
            CAST(COUNT(DISTINCT CASE WHEN is_consensus = 1 THEN token_address || ':' || created_at END) AS REAL) /
            NULLIF(COUNT(DISTINCT token_address || ':' || created_at), 0) AS rate
        FROM signal_aggregation
        WHERE created_at >= datetime('now', '-24 hours')
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap_or(0.0);

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

    // Get active clusters
    let active_clusters = if let Some(ref agg) = state.signal_aggregator {
        get_active_clusters(agg, &sqlite_pool(&state.db)?).await
    } else {
        Vec::new()
    };

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
    let pool = sqlite_pool(&state.db)?;
    let clusters = if let Some(ref agg) = state.signal_aggregator {
        get_active_clusters(agg, &pool).await
    } else {
        Vec::new()
    };

    let total_wallets = clusters.iter().map(|c| c.wallets.len()).sum::<usize>();

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
        clusters,
        total_wallets,
        clustering_metrics,
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

    let pool = sqlite_pool(&state.db)?;

    // Query signals in the aggregation window
    let signal_rows = sqlx::query(
        r#"
        SELECT
            token_address,
            wallet_address,
            direction,
            amount_sol,
            consensus_wallet_count,
            datetime(created_at) as created_at
        FROM signal_aggregation
        WHERE created_at >= datetime('now', '-5 minutes')
        ORDER BY created_at DESC
        "#,
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

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
/// Returns signal quality score, distribution buckets, rejection rate,
/// and quality trend over time.
pub async fn get_signal_quality(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<SignalQualityParams>,
) -> Result<Json<SignalQualityResponse>, AppError> {
    let pool = sqlite_pool(&state.db)?;

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
    let cutoff_str = cutoff.to_rfc3339();

    // Total signals in time range
    let total_signals: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE created_at >= ?")
            .bind(&cutoff_str)
            .fetch_one(&pool)
            .await
            .unwrap_or(0);

    // Accepted vs rejected signals
    let (accepted_signals, rejected_signals): (i64, i64) = sqlx::query_as(
        r#"
            SELECT
                COUNT(CASE WHEN status IN ('ACTIVE', 'CLOSED') THEN 1 END) as accepted,
                COUNT(CASE WHEN status IN ('FAILED', 'DEAD_LETTER') THEN 1 END) as rejected
            FROM trades WHERE created_at >= ?
            "#,
    )
    .bind(&cutoff_str)
    .fetch_one(&pool)
    .await
    .unwrap_or((0, 0));

    // Current quality score (average WQS of wallets that sent signals)
    let current_quality_score: f64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(AVG(w.wqs_score), 50.0)
        FROM trades t
        LEFT JOIN wallets w ON t.wallet_address = w.address
        WHERE t.created_at >= ?
        "#,
    )
    .bind(&cutoff_str)
    .fetch_one(&pool)
    .await
    .unwrap_or(50.0);

    // Rejection rate
    let rejection_rate = if total_signals > 0 {
        rejected_signals as f64 / total_signals as f64
    } else {
        0.0
    };

    let hours = match range.as_str() {
        "1h" => 1,
        "6h" => 6,
        "24h" => 24,
        "7d" => 168,
        _ => 24,
    };

    // Quality distribution buckets
    let quality_distribution = build_quality_distribution(&pool, &cutoff_str).await;

    // Average quality trend (hourly data points)
    let average_quality_trend = build_quality_trend(&pool, &cutoff_str, hours).await;

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
    let pool = sqlite_pool(&state.db)?;

    // Query per-wallet signal statistics (last 7 days)
    let source_rows = sqlx::query(
        r#"
        SELECT
            t.wallet_address as source,
            CAST(COUNT(*) AS INTEGER) as signal_count,
            COALESCE(w.wqs_score, 50.0) as average_quality,
            CAST(COUNT(CASE WHEN t.status IN ('ACTIVE', 'CLOSED') THEN 1 END) AS REAL) / CAST(COUNT(*) AS REAL) as acceptance_rate,
            MAX(t.created_at) as last_signal_at
        FROM trades t
        LEFT JOIN wallets w ON t.wallet_address = w.address
        WHERE t.created_at >= datetime('now', '-7 days')
        GROUP BY t.wallet_address
        ORDER BY COUNT(*) DESC
        LIMIT 50
        "#,
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

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

/// Get active clusters from signal aggregator and database
async fn get_active_clusters(
    aggregator: &crate::monitoring::signal_aggregator::SignalAggregator,
    db: &sqlx::Pool<sqlx::Sqlite>,
) -> Vec<Cluster> {
    // Query active wallets from database
    let active_wallets = sqlx::query_scalar::<_, String>(
        r#"
        SELECT address
        FROM wallets
        WHERE status = 'ACTIVE'
        LIMIT 50
        "#
    )
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let mut clusters = Vec::new();

    // For each active wallet, get its cluster
    for wallet in active_wallets {
        let related = aggregator.get_wallet_cluster(&wallet).await;

        if !related.is_empty() {
            let mut cluster_wallets = vec![wallet.to_string()];
            cluster_wallets.extend(related);

            // Get average WQS for the cluster - use COALESCE to handle NULL
            let avg_wqs: f64 = sqlx::query_scalar(
                r#"
                SELECT COALESCE(AVG(wqs_score), 50.0)
                FROM wallets
                WHERE address IN (
                    SELECT value FROM json_each(?)
                )
                "#,
            )
            .bind(serde_json::to_string(&cluster_wallets).unwrap_or_default())
            .fetch_one(db)
            .await
            .unwrap_or(50.0);

            clusters.push(Cluster {
                id: format!("cluster_{}", &wallet[..8.min(wallet.len())]),
                wallets: cluster_wallets,
                signal_count: 2, // Placeholder
                avg_wqs,
                last_activity: chrono::Utc::now().to_rfc3339(),
                coherence: 0.7, // Placeholder
            });
        }
    }

    // Deduplicate clusters by wallet set
    let mut unique_clusters: Vec<Cluster> = Vec::new();
    let mut seen_wallets: std::collections::HashSet<String> = std::collections::HashSet::new();

    for cluster in clusters {
        let is_new = cluster.wallets.iter().any(|w| !seen_wallets.contains(w));
        if is_new {
            for w in &cluster.wallets {
                seen_wallets.insert(w.clone());
            }
            unique_clusters.push(cluster);
        }
    }

    unique_clusters
}

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

/// Build quality distribution buckets
async fn build_quality_distribution(
    db: &sqlx::Pool<sqlx::Sqlite>,
    cutoff_str: &str,
) -> Vec<QualityBucket> {
    let buckets = [
        ("0-0.2", 0.0, 20.0),
        ("0.2-0.4", 20.0, 40.0),
        ("0.4-0.6", 40.0, 60.0),
        ("0.6-0.8", 60.0, 80.0),
        ("0.8-1.0", 80.0, 100.0),
    ];

    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT t.wallet_address) FROM trades t
         LEFT JOIN wallets w ON t.wallet_address = w.address
         WHERE t.created_at >= ?",
    )
    .bind(cutoff_str)
    .fetch_one(db)
    .await
    .unwrap_or(1);

    let mut distribution = Vec::new();

    for (range, min_score, max_score) in buckets {
        let count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(DISTINCT t.wallet_address)
            FROM trades t
            LEFT JOIN wallets w ON t.wallet_address = w.address
            WHERE t.created_at >= ?
            AND COALESCE(w.wqs_score, 50.0) >= ? AND COALESCE(w.wqs_score, 50.0) < ?
            "#,
        )
        .bind(cutoff_str)
        .bind(min_score)
        .bind(max_score)
        .fetch_one(db)
        .await
        .unwrap_or(0);

        let percentage = if total > 0 {
            (count as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        distribution.push(QualityBucket {
            range: range.to_string(),
            count,
            percentage,
        });
    }

    distribution
}

/// Build quality trend data (hourly points)
async fn build_quality_trend(
    db: &sqlx::Pool<sqlx::Sqlite>,
    _cutoff_str: &str,
    hours: i64,
) -> Vec<QualityTrendPoint> {
    let mut trend = Vec::new();

    // For simplicity, we'll just query hourly averages for the last N hours
    // In production, this might use a more sophisticated time-series approach
    for hour_offset in (0..hours).rev() {
        let hour_start = format!("datetime('now', '-{} hours')", hour_offset + 1);
        let hour_end = format!("datetime('now', '-{} hours')", hour_offset);

        let avg_score: f64 = sqlx::query_scalar(&format!(
            r#"
            SELECT COALESCE(AVG(w.wqs_score), 50.0)
            FROM trades t
            LEFT JOIN wallets w ON t.wallet_address = w.address
            WHERE t.created_at >= {} AND t.created_at < {}
            "#,
            hour_start, hour_end
        ))
        .fetch_one(db)
        .await
        .unwrap_or(50.0);

        let timestamp = chrono::Utc::now() - chrono::Duration::hours(hour_offset);

        trend.push(QualityTrendPoint {
            timestamp: timestamp.to_rfc3339(),
            average_score: avg_score,
        });
    }

    trend
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
            // This is a divergence pattern - wallets disagree on direction
            let divergence_type = if buyers.len() > sellers.len() {
                "directional_bullish".to_string() // More buyers than sellers
            } else if sellers.len() > buyers.len() {
                "directional_bearish".to_string() // More sellers than buyers
            } else {
                "timing".to_string() // Equal split - timing divergence
            };

            // Create wallet clusters for divergent wallets
            let wallets_clustered = vec![WalletCluster {
                cluster_id: format!("holders_{}", token_address[..8].to_string()),
                wallet_addresses: buyers.iter().map(|b| b.wallet_address.clone()).collect(),
                signal: "BUY".to_string(),
            }];

            let wallets_divergent = vec![WalletCluster {
                cluster_id: format!("sellers_{}", token_address[..8].to_string()),
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
