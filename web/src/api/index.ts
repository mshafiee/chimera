export { apiClient, getApiError } from './client'
export { useHealth } from './health'
export { usePositions, usePosition } from './positions'
export { useWallets, useWallet, useUpdateWallet } from './wallets'
export { useTrades, exportTrades } from './trades'
export { useConfig, useUpdateConfig, useResetCircuitBreaker, useTripCircuitBreaker } from './config'
export { useDeadLetterQueue, useConfigAudit } from './incidents'
export { usePerformanceMetrics, useStrategyPerformance } from './metrics'

// New API clients
export {
  useScoutStatus,
  useWQSDistribution,
  useScoutMetrics,
  triggerScoutRun,
} from './scout'
export {
  useSignalQuality,
  useSignalSources,
  useSignalConsensus,
} from './signals'
export {
  useMarketRegime,
  useMarketConditions,
} from './market'
export {
  usePortfolioRisk,
  useStopLossMetrics,
  useProfitTargetMetrics,
  usePositionSizeAnalysis,
} from './risk'
export {
  useReconciliationStatus,
  useReconciliationHistory,
  useReconciliationStats,
  useTriggerReconciliation,
  useResolveDiscrepancy,
} from './reconciliation'
export {
  useTradeLatency,
  useRPCLatency,
  useDatabasePerformance,
  useRequestRate,
  useCostAnalysis,
} from './performance'
export {
  useResourceUsage,
  useSecretRotation,
  useRateLimitStatus,
  useSystemLogs,
  useHealthCheckDetails,
} from './operations'
export {
  useConsensus,
  useWalletClustering,
  useSignalAggregation,
} from './consensus'

// Type exports
export type {
  ScoutStatusResponse,
  WQSBucket,
  PromotionItem,
  RejectionItem,
  WQSDistributionResponse,
  ScoutMetricsResponse,
} from './scout'
export type {
  SignalQualityResponse,
  QualityBucket,
  SignalSourceResponse,
  SignalSource,
  SignalConsensusResponse,
  DivergenceAlert,
  ConsensusSignal,
} from './signals'
export type {
  MarketRegime,
  MarketRegimeResponse,
  MarketConditionsResponse,
} from './market'
export type {
  PortfolioRiskResponse,
  ConcentrationData,
  ExposureData,
  DrawdownData,
  StopLossMetricsResponse,
  ProfitTargetMetricsResponse,
  PositionSizeAnalysisResponse,
} from './risk'
export type {
  ReconciliationStatusResponse,
  Discrepancy,
  ReconciliationHistoryResponse,
  ReconciliationStatsResponse,
} from './reconciliation'
export type {
  TradeLatencyResponse,
  RPCLatencyResponse,
  DatabasePerformanceResponse,
  RequestRateResponse,
  CostAnalysisResponse,
} from './performance'
export type {
  ResourceUsageResponse,
  SecretRotationResponse,
  RateLimitStatusResponse,
  SystemLogsResponse,
  HealthCheckDetailsResponse,
} from './operations'
export type {
  ConsensusResponse,
  Cluster,
  ConsensusSignal,
  DivergenceAlert,
  WalletClusteringResponse,
  SignalAggregationResponse,
} from './consensus'
