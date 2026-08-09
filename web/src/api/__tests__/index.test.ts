import { describe, it, expect } from 'vitest'
import * as api from '../index'

describe('api barrel', () => {
  it('re-exports the client helpers', () => {
    expect(api.apiClient).toBeDefined()
    expect(api.getApiError).toBeDefined()
  })

  it('re-exports query hooks', () => {
    const hooks = [
      api.useHealth,
      api.usePositions,
      api.usePosition,
      api.useWallets,
      api.useWallet,
      api.useUpdateWallet,
      api.useTrades,
      api.useConfig,
      api.useUpdateConfig,
      api.useResetCircuitBreaker,
      api.useTripCircuitBreaker,
      api.useDeadLetterQueue,
      api.useConfigAudit,
      api.usePerformanceMetrics,
      api.useStrategyPerformance,
      api.useBalanceAndNAV,
      api.useNavHistory,
      api.useScoutStatus,
      api.useWQSDistribution,
      api.useScoutMetrics,
      api.useSignalQuality,
      api.useSignalSources,
      api.useSignalConsensus,
      api.useMarketRegime,
      api.useMarketConditions,
      api.usePortfolioRisk,
      api.useStopLossMetrics,
      api.useProfitTargetMetrics,
      api.usePositionSizeAnalysis,
      api.useReconciliationStatus,
      api.useReconciliationHistory,
      api.useReconciliationStats,
      api.useTriggerReconciliation,
      api.useResolveDiscrepancy,
      api.useTradeLatency,
      api.useRPCLatency,
      api.useDatabasePerformance,
      api.useRequestRate,
      api.useCostAnalysis,
      api.useResourceUsage,
      api.useSecretRotation,
      api.useRateLimitStatus,
      api.useHealthCheckDetails,
      api.useConsensus,
      api.useWalletClustering,
      api.useConsensusSignalAggregation,
      api.useWebhookStats,
      api.useWebhookAuditLog,
      api.useBulkRegisterWebhooks,
      api.useBulkCleanupWebhooks,
      api.useReconcileWebhooks,
      api.useHealthCheckWebhooks,
      api.useToggleWebhook,
      api.useRetryWebhook,
      api.useWalletMonitoringStates,
    ]
    for (const hook of hooks) {
      expect(typeof hook).toBe('function')
    }
  })

  it('re-exports the plain functions', () => {
    expect(typeof api.exportTrades).toBe('function')
    expect(typeof api.retryDeadLetterItem).toBe('function')
    expect(typeof api.triggerScoutRun).toBe('function')
  })
})
