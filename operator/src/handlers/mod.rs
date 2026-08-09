//! HTTP handlers for Chimera Operator

mod api;
mod auth;
mod health;
mod market;
mod monitoring;
mod operations;
pub mod profitability;
mod risk;
mod scout;
mod signals;
mod webhook;
mod webhook_lifecycle;
mod ws;

// Explicit re-export lists (no globs) so the public surface stays intentional
// and auditable, and name collisions surface here instead of at call sites.
pub use api::{
    clear_monitoring_caches, debug_backtest_smoke, export_trades, get_config, get_cost_metrics,
    get_database_performance, get_performance_metrics, get_position, get_request_rate,
    get_rpc_latency, get_shadow_leaderboard, get_strategy_performance, get_trade_latency,
    get_wallet, get_wallet as _get_wallet, list_config_audit, list_dead_letter_queue,
    list_positions, list_trades, list_wallets, require_role_from_request, reset_circuit_breaker,
    resolve_discrepancy, retry_dead_letter_item, trigger_reconciliation, trip_circuit_breaker,
    update_config, update_reconciliation_metrics, update_secret_rotation_metrics, update_wallet,
    ApiState, CachePerformanceStats, CircuitBreakerConfig, CircuitBreakerResetResponse,
    ConfigAuditQuery, ConfigAuditResponse, ConfigResponse, ConnectionPoolStats,
    CostMetricsResponse, DailySummaryConfigResponse, DatabasePerformanceResponse, DeadLetterQuery,
    DeadLetterResponse, DebugBacktestSmokeRequest, DebugBacktestSmokeResponse, DiscrepancyResponse,
    DiscrepancyTypeStatsResponse, JitoTipConfig, MevProtectionConfigResponse,
    MonitoringConfigResponse, NotificationRulesConfigResponse, NotificationsConfigResponse,
    PerformanceMetricsResponse, PositionSizingConfigResponse, PositionsQuery, PositionsResponse,
    ProfitManagementConfigResponse, QueueConfigResponse, RPCEndpointLatency, RPCLatencyResponse,
    RateLimitInfo, ReconciliationHistoryQuery, ReconciliationHistoryResponse,
    ReconciliationMetricsUpdate, ReconciliationRunResponse, ReconciliationStatsQuery,
    ReconciliationStatsResponse, ReconciliationStatusQuery, ReconciliationStatusResponse,
    RequestRateResponse, ResolveDiscrepancyRequest, ResolveDiscrepancyResponse, RetryResponse,
    RpcStatus, SecretRotationMetricsUpdate, ShadowLeaderboardRow, StrategyAllocation,
    StrategyConfigResponse, StrategyPerformanceResponse, TradeLatencyResponse, TradesQuery,
    TradesResponse, TriggerReconciliationResponse, UpdateCircuitBreakerConfig, UpdateConfigRequest,
    UpdateConfigRequest as _UpdateConfigRequest, UpdateDailySummaryConfig,
    UpdateMevProtectionConfig, UpdateMonitoringConfig, UpdateNotificationRulesConfig,
    UpdateNotificationsConfig, UpdatePositionSizingConfig, UpdateProfitManagementConfig,
    UpdateQueueConfig, UpdateStrategyAllocation, UpdateStrategyConfig, UpdateTelegramConfig,
    UpdateTokenSafetyConfig, UpdateWalletRequest, WalletUpdateResponse, WalletsQuery,
    WalletsResponse,
};
pub use api::{get_reconciliation_history, get_reconciliation_stats, get_reconciliation_status};
pub use auth::{
    refresh_token, wallet_auth, RefreshTokenRequest, RefreshTokenResponse, WalletAuthRequest,
    WalletAuthResponse, WalletAuthState,
};
pub use health::{
    health_check, health_simple, AppState, CircuitBreakerHealth, ComponentHealth, HealthResponse,
    HealthStatus, PriceCacheHealth,
};
pub use market::{
    get_market_conditions, get_market_regime, MarketConditionsResponse, MarketRegimeResponse,
    PerformanceByRegime, RecommendedAllocation, RegimeHistoryPoint,
};
pub use monitoring::{
    disable_wallet_monitoring, enable_wallet_monitoring, get_monitoring_status,
    get_wallet_monitoring_states, helius_webhook_handler, MonitoringStatus,
    WalletMonitoringStateItem, WalletMonitoringStateResponse,
};
pub use operations::{
    get_health_check_details, get_rate_limit_status, get_resources, get_secrets, CheckStatus,
    DegradationStatus, EndpointStatus, EventStatus, HealthCheck, HealthCheckDetailsResponse,
    MetricStatus, NetworkMetric, OperationsState, OverallHealthStatus, OverallStatus,
    RateLimitEndpoint, RateLimitStatusResponse, ResourceMetric, ResourceUsageResponse,
    RotationEvent, RotationStatus, SecretRotationResponse,
};
pub use profitability::{
    count_invalid_pnl, count_missing_outcomes, evaluate_gates, fetch_outcomes,
    profitability_verdict, BiasGate, CachedVerdict, CohortGate, CompletenessGate, DrawdownGate,
    GateValue, IntegrityGate, LossGate, NetReturnGate, Outcome, VerdictGates, VerdictQuery,
    VerdictResponse,
};
pub use risk::{
    calculate_hhi, classify_token_sector, determine_heat_status, get_nav_history,
    get_portfolio_risk, get_position_size_analysis, get_profit_target_metrics,
    get_stop_loss_metrics, ConcentrationData, DrawdownData, ExposureData, NavHistoryPoint,
    NavHistoryResponse, PortfolioRiskResponse, PositionSizeAnalysisResponse, ProfitTargetHit,
    ProfitTargetMetricsResponse, SectorConcentration, SizeBucket, StopLossActivation,
    StopLossMetricsResponse, StrategyProfitTargetData, StrategyStopLossData, TimeRangeQuery,
    TokenConcentration,
};
pub use scout::{
    get_budget_status, get_cache_stats, get_conviction_allocation, get_scout_metrics,
    get_scout_status, get_wqs_distribution, trigger_scout_run, ActivityDistribution,
    AllocationSummary, BudgetBreakdown, BudgetForecast, BudgetStatusResponse, CacheStatsResponse,
    ConvictionAllocationResponse, OptimizationSuggestion, PromotionItem, RejectionItem,
    ScoutMetricsResponse, ScoutRunResponse, ScoutStatusResponse, ScoutTimeRangeQuery, WQSBucket,
    WQSDistributionResponse, WalletAnalysisBreakdown, WalletLevelStats,
};
pub use signals::{
    get_consensus, get_signal_aggregation, get_signal_quality, get_signal_sources,
    get_wallet_clustering, AggregatedSignal, Cluster, ClusteringMetrics, ConsensusResponse,
    ConsensusSignal, DivergenceAlert, ExecutionResult, QualityBucket, QualityTrendPoint,
    SignalAggregationResponse, SignalQualityParams, SignalQualityResponse, SignalSource,
    SignalSourcesResponse, WalletCluster, WalletClusteringResponse,
};
pub use webhook::{webhook_handler, WebhookRequest, WebhookResponse, WebhookState, WebhookStatus};
pub use webhook_lifecycle::{
    bulk_cleanup_webhooks, bulk_register_webhooks, get_webhook_audit_log, get_webhook_stats,
    manual_health_check, manual_reconcile_webhooks, retry_webhook_registration,
    toggle_wallet_webhook, ApiResponse, AuditQuery, BulkCleanupRequest, BulkRegisterRequest,
    ToggleWebhookRequest,
};
pub use ws::{
    ws_handler, AlertData, HealthUpdateData, PositionUpdateData, TradeUpdateData, WsEvent, WsState,
};
