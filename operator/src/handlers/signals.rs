//! Signal consensus and aggregation API handlers
//!
//! Provides endpoints for:
//! - Consensus detection overview
//! - Wallet clustering analysis
//! - Signal aggregation status

use axum::{extract::Query, extract::State, Json};
use serde::Serialize;
use std::sync::Arc;

use super::api::ApiState;
use crate::error::AppError;

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
    // Query database for recent consensus signals
    let recent_signals = sqlx::query_as!(
        SignalAggRow,
        r#"
        SELECT
            token_address,
            wallet_address,
            direction,
            amount_sol,
            consensus_wallet_count,
            datetime(created_at) as "created_at!"
        FROM signal_aggregation
        WHERE is_consensus = 1
        ORDER BY created_at DESC
        LIMIT 20
        "#
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    // Calculate consensus rate (consensus signals / total signals in last 24h)
    let consensus_rate = sqlx::query_scalar!(
        r#"
        SELECT
            CAST(COUNT(DISTINCT CASE WHEN is_consensus = 1 THEN token_address || ':' || created_at END) AS REAL) /
            NULLIF(COUNT(DISTINCT token_address || ':' || created_at), 0) AS rate
        FROM signal_aggregation
        WHERE created_at >= datetime('now', '-24 hours')
        "#
    )
    .fetch_one(&state.db)
    .await
    .ok()
    .flatten()
    .unwrap_or(0.0);

    // Group by token for consensus signals
    let mut consensus_signals: std::collections::HashMap<String, Vec<SignalAggRow>> =
        std::collections::HashMap::new();
    for row in recent_signals {
        consensus_signals
            .entry(row.token_address.clone())
            .or_insert_with(Vec::new)
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
        get_active_clusters(agg, &state.db).await
    } else {
        Vec::new()
    };

    Ok(Json(ConsensusResponse {
        consensus_rate,
        avg_clustering_coefficient,
        active_clusters,
        recent_signals,
        divergence_alerts: Vec::new(), // TODO: Implement divergence detection
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
    let clusters = if let Some(ref agg) = state.signal_aggregator {
        get_active_clusters(agg, &state.db).await
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
        modularity: 0.3, // Placeholder
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

    // Query signals in the aggregation window
    let signals = sqlx::query_as!(
        SignalAggRow,
        r#"
        SELECT
            token_address,
            wallet_address,
            direction,
            amount_sol,
            consensus_wallet_count,
            datetime(created_at) as "created_at!"
        FROM signal_aggregation
        WHERE created_at >= datetime('now', '-5 minutes')
        ORDER BY created_at DESC
        "#
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let total_signals = signals.len();
    let unique_tokens = signals
        .iter()
        .map(|s| s.token_address.clone())
        .collect::<std::collections::HashSet<_>>()
        .len();

    // Aggregate by token
    let mut token_aggregates: std::collections::HashMap<
        String,
        AggregatedSignalData,
    > = std::collections::HashMap::new();

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
    // Parse time range
    let range = params.range;
    let time_filter = match range.as_str() {
        "1h" => "datetime('now', '-1 hour')",
        "6h" => "datetime('now', '-6 hours')",
        "24h" => "datetime('now', '-24 hours')",
        "7d" => "datetime('now', '-7 days')",
        _ => "datetime('now', '-24 hours')", // Default to 24h
    };

    // Total signals in time range
    let total_signals: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM trades WHERE created_at >= {}",
        time_filter
    ))
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    // Accepted vs rejected signals
    let (accepted_signals, rejected_signals): (i64, i64) =
        sqlx::query_as(&format!(
            r#"
            SELECT
                COUNT(CASE WHEN status IN ('ACTIVE', 'CLOSED') THEN 1 END) as accepted,
                COUNT(CASE WHEN status IN ('FAILED', 'DEAD_LETTER') THEN 1 END) as rejected
            FROM trades WHERE created_at >= {}
            "#,
            time_filter
        ))
        .fetch_one(&state.db)
        .await
        .unwrap_or((0, 0));

    // Current quality score (average WQS of wallets that sent signals)
    let current_quality_score: f64 = sqlx::query_scalar(&format!(
        r#"
        SELECT COALESCE(AVG(w.wqs_score), 50.0)
        FROM trades t
        LEFT JOIN wallets w ON t.wallet_address = w.address
        WHERE t.created_at >= {}
        "#,
        time_filter
    ))
    .fetch_one(&state.db)
    .await
    .unwrap_or(50.0);

    // Rejection rate
    let rejection_rate = if total_signals > 0 {
        rejected_signals as f64 / total_signals as f64
    } else {
        0.0
    };

    // Quality distribution buckets
    let quality_distribution = build_quality_distribution(&state.db, time_filter).await;

    // Average quality trend (hourly data points)
    let average_quality_trend = build_quality_trend(&state.db, time_filter).await;

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
    // Query per-wallet signal statistics (last 7 days)
    let sources_raw = sqlx::query_as!(
        SignalSourceRow,
        r#"
        SELECT
            t.wallet_address as "source!",
            CAST(COUNT(*) AS INTEGER) as "signal_count!",
            COALESCE(w.wqs_score, 50.0) as "average_quality!",
            CAST(COUNT(CASE WHEN t.status IN ('ACTIVE', 'CLOSED') THEN 1 END) AS REAL) / CAST(COUNT(*) AS REAL) as "acceptance_rate!",
            MAX(t.created_at) as "last_signal_at!"
        FROM trades t
        LEFT JOIN wallets w ON t.wallet_address = w.address
        WHERE t.created_at >= datetime('now', '-7 days')
        GROUP BY t.wallet_address
        ORDER BY COUNT(*) DESC
        LIMIT 50
        "#
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let total_signals = sources_raw.iter().map(|s| s.signal_count).sum::<i64>();

    let sources: Vec<SignalSource> = sources_raw
        .into_iter()
        .map(|row| {
            let dt = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
                row.last_signal_at,
                chrono::Utc,
            );
            SignalSource {
                source: row.source,
                signal_count: row.signal_count,
                average_quality: row.average_quality,
                acceptance_rate: row.acceptance_rate,
                last_signal_at: dt.to_rfc3339(),
            }
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
async fn calculate_clustering_coefficient(_aggregator: &crate::monitoring::signal_aggregator::SignalAggregator) -> f64 {
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
    let active_wallets = sqlx::query_scalar!(
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
            let mut cluster_wallets = vec![wallet.clone()];
            cluster_wallets.extend(related);

            // Get average WQS for the cluster - use COALESCE to handle NULL
            let avg_wqs: f64 = sqlx::query_scalar(
                r#"
                SELECT COALESCE(AVG(wqs_score), 50.0)
                FROM wallets
                WHERE address IN (
                    SELECT value FROM json_each(?)
                )
                "#
            )
            .bind(serde_json::to_string(&cluster_wallets).unwrap_or_default())
            .fetch_one(db)
            .await
            .unwrap_or(50.0);

            clusters.push(Cluster {
                id: format!("cluster_{}", wallet[..8.min(wallet.len())].to_string()),
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
    last_signal_at: chrono::NaiveDateTime,
}

/// Build quality distribution buckets
async fn build_quality_distribution(
    db: &sqlx::Pool<sqlx::Sqlite>,
    time_filter: &str,
) -> Vec<QualityBucket> {
    let buckets = [
        ("0-0.2", 0.0, 20.0),
        ("0.2-0.4", 20.0, 40.0),
        ("0.4-0.6", 40.0, 60.0),
        ("0.6-0.8", 60.0, 80.0),
        ("0.8-1.0", 80.0, 100.0),
    ];

    // Get total count for percentage calculation
    let total: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(DISTINCT t.wallet_address) FROM trades t
         LEFT JOIN wallets w ON t.wallet_address = w.address
         WHERE t.created_at >= {}",
        time_filter
    ))
    .fetch_one(db)
    .await
    .unwrap_or(1); // Avoid division by zero

    let mut distribution = Vec::new();

    for (range, min_score, max_score) in buckets {
        let count: i64 = sqlx::query_scalar(&format!(
            r#"
            SELECT COUNT(DISTINCT t.wallet_address)
            FROM trades t
            LEFT JOIN wallets w ON t.wallet_address = w.address
            WHERE t.created_at >= {}
            AND COALESCE(w.wqs_score, 50.0) >= {} AND COALESCE(w.wqs_score, 50.0) < {}
            "#,
            time_filter, min_score, max_score
        ))
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
    time_filter: &str,
) -> Vec<QualityTrendPoint> {
    // Determine number of data points based on time range
    let hours = if time_filter.contains("1 hour") {
        1
    } else if time_filter.contains("6 hours") {
        6
    } else if time_filter.contains("7 days") {
        168 // 7 * 24
    } else {
        24 // Default 24h
    };

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

        let timestamp = chrono::Utc::now() - chrono::Duration::hours(hour_offset as i64);

        trend.push(QualityTrendPoint {
            timestamp: timestamp.to_rfc3339(),
            average_score: avg_score,
        });
    }

    trend
}
