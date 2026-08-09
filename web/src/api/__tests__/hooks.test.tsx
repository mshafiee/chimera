import React from 'react'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { renderHook, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import type { ReactNode } from 'react'

const apiClientMock = vi.hoisted(() => ({
  get: vi.fn(),
  post: vi.fn(),
  put: vi.fn(),
}))

vi.mock('../client', () => ({
  apiClient: apiClientMock,
  getApiError: (error: unknown) =>
    error instanceof Error ? error.message : 'mock api error',
}))

vi.mock('sonner', () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
    info: vi.fn(),
  },
}))

import { useHealth } from '../health'
import { usePositions, usePosition } from '../positions'
import { useWallets, useWallet, useUpdateWallet } from '../wallets'
import { useTrades, exportTrades } from '../trades'
import { useConfig, useUpdateConfig, useResetCircuitBreaker, useTripCircuitBreaker } from '../config'
import { useDeadLetterQueue, useConfigAudit, retryDeadLetterItem } from '../incidents'
import { usePerformanceMetrics, useStrategyPerformance, useCostMetrics } from '../metrics'
import { useBalanceAndNAV } from '../balance'
import { useNavHistory } from '../navHistory'
import {
  useScoutStatus,
  useWQSDistribution,
  useScoutMetrics,
  triggerScoutRun,
  useBudgetStatus,
  useCacheStats,
  useConvictionAllocation,
} from '../scout'
import { useSignalQuality, useSignalSources, useSignalConsensus, useSignalAggregation, useSignalClustering } from '../signals'
import { useMarketRegime, useMarketConditions } from '../market'
import { usePortfolioRisk, useStopLossMetrics, useProfitTargetMetrics, usePositionSizeAnalysis } from '../risk'
import {
  useReconciliationStatus,
  useReconciliationHistory,
  useReconciliationStats,
  useTriggerReconciliation,
  useResolveDiscrepancy,
} from '../reconciliation'
import {
  useTradeLatency,
  useRPCLatency,
  useDatabasePerformance,
  useRequestRate,
  useCostAnalysis,
} from '../performance'
import {
  useResourceUsage,
  useSecretRotation,
  useRateLimitStatus,
  useHealthCheckDetails,
} from '../operations'
import { useConsensus, useWalletClustering, useSignalAggregation as useConsensusSignalAggregation } from '../consensus'

let queryClient: QueryClient

function createWrapper() {
  queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  })
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  )
}

const wrapper = createWrapper()

beforeEach(() => {
  vi.resetAllMocks()
  queryClient.clear()
  apiClientMock.get.mockResolvedValue({ data: {} })
  apiClientMock.post.mockResolvedValue({ data: {}, status: 200 })
  apiClientMock.put.mockResolvedValue({ data: {} })
})

describe('health', () => {
  it('fetches health status', async () => {
    apiClientMock.get.mockResolvedValue({ data: { status: 'healthy' } })
    const { result } = renderHook(() => useHealth(), { wrapper })
    await waitFor(() => expect(result.current.data).toEqual({ status: 'healthy' }))
    expect(apiClientMock.get).toHaveBeenCalledWith('/health', expect.anything())
  })
})

describe('positions', () => {
  it('fetches positions with an optional state filter', async () => {
    const data = { positions: [], total: 0, total_unrealized_pnl_sol: null }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => usePositions('ACTIVE'), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
    expect(apiClientMock.get).toHaveBeenCalledWith('/positions', expect.anything())
  })

  it('fetches positions without a state filter', async () => {
    const { result } = renderHook(() => usePositions(), { wrapper })
    await waitFor(() => expect(result.current.data).toEqual({}))
  })

  it('fetches a single position', async () => {
    const data = { id: 1, trade_uuid: 'abc' }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => usePosition('abc'), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
    expect(apiClientMock.get).toHaveBeenCalledWith('/positions/abc', expect.anything())
  })

  it('does not fetch a position without a trade uuid', async () => {
    const { result } = renderHook(() => usePosition(''), { wrapper })
    await waitFor(() => expect(result.current.fetchStatus).toBe('idle'))
  })
})

describe('wallets', () => {
  it('fetches wallets with a status filter', async () => {
    const data = { wallets: [], total: 0 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useWallets('ACTIVE'), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
    expect(apiClientMock.get).toHaveBeenCalledWith('/wallets', expect.anything())
  })

  it('fetches wallets without a status filter', async () => {
    const { result } = renderHook(() => useWallets(), { wrapper })
    await waitFor(() => expect(result.current.data).toEqual({}))
  })

  it('fetches a single wallet', async () => {
    const data = { id: 1, address: 'wallet1' }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useWallet('wallet1'), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
    expect(apiClientMock.get).toHaveBeenCalledWith('/wallets/wallet1', expect.anything())
  })

  it('does not fetch a wallet without an address', async () => {
    const { result } = renderHook(() => useWallet(''), { wrapper })
    await waitFor(() => expect(result.current.fetchStatus).toBe('idle'))
  })

  it('updates a wallet via mutation', async () => {
    const data = { success: true, wallet: null, message: 'ok' }
    apiClientMock.put.mockResolvedValue({ data })
    const { result } = renderHook(() => useUpdateWallet(), { wrapper })
    await result.current.mutateAsync({ address: 'wallet1', status: 'ACTIVE', reason: 'x' })
    expect(apiClientMock.put).toHaveBeenCalledWith('/wallets/wallet1', {
      status: 'ACTIVE',
      reason: 'x',
    })
  })

  it('throws when the wallet update fails', async () => {
    apiClientMock.put.mockResolvedValue({ data: { success: false, message: 'nope' } })
    const { result } = renderHook(() => useUpdateWallet(), { wrapper })
    await expect(
      result.current.mutateAsync({ address: 'w', status: 'REJECTED' })
    ).rejects.toThrow('nope')
  })
})

describe('trades', () => {
  it('fetches trades with params', async () => {
    const data = { trades: [], total: 0, limit: 25, offset: 0 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useTrades({ status: 'CLOSED', limit: 25 }), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('exports trades with a content-disposition filename', async () => {
    const createObjectURL = vi.fn(() => 'blob:url')
    const revokeObjectURL = vi.fn()
    vi.stubGlobal('URL', { createObjectURL, revokeObjectURL })
    const clickSpy = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {})

    apiClientMock.get.mockResolvedValue({
      data: new Blob(['a,b']),
      headers: { 'content-disposition': 'attachment; filename="trades.csv"' },
    })
    await exportTrades({ from: '2025-01-01' }, 'csv')
    expect(apiClientMock.get).toHaveBeenCalledWith('/trades/export', expect.anything())
    expect(clickSpy).toHaveBeenCalled()
    await new Promise((r) => setTimeout(r, 10))
    vi.unstubAllGlobals()
    clickSpy.mockRestore()
  })

  it('exports trades with a default filename when no content-disposition header', async () => {
    const createObjectURL = vi.fn(() => 'blob:url')
    const revokeObjectURL = vi.fn()
    vi.stubGlobal('URL', { createObjectURL, revokeObjectURL })
    const clickSpy = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {})
    apiClientMock.get.mockResolvedValue({ data: new Blob(['a']), headers: {} })
    await exportTrades({}, 'json')
    expect(clickSpy).toHaveBeenCalled()
    await new Promise((r) => setTimeout(r, 10))
    vi.unstubAllGlobals()
    clickSpy.mockRestore()
  })

  it('throws when the export fails', async () => {
    apiClientMock.get.mockRejectedValue(new Error('export exploded'))
    await expect(exportTrades({})).rejects.toThrow('export exploded')
  })
})

describe('config', () => {
  it('fetches config', async () => {
    const data = { circuit_breakers: {} }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useConfig(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
    expect(apiClientMock.get).toHaveBeenCalledWith('/config', expect.anything())
  })

  it('updates config', async () => {
    const data = { circuit_breakers: {} }
    apiClientMock.put.mockResolvedValue({ data })
    const { result } = renderHook(() => useUpdateConfig(), { wrapper })
    await result.current.mutateAsync({ strategy: { max_position_sol: 5 } })
    expect(apiClientMock.put).toHaveBeenCalledWith('/config', { strategy: { max_position_sol: 5 } })
  })

  it('throws when the update body is empty', async () => {
    const { result } = renderHook(() => useUpdateConfig(), { wrapper })
    await expect(result.current.mutateAsync({})).rejects.toThrow('Nothing to update')
  })

  it('resets the circuit breaker', async () => {
    const data = { success: true, message: 'ok', previous_state: 'x', new_state: 'y' }
    apiClientMock.post.mockResolvedValue({ data })
    const { result } = renderHook(() => useResetCircuitBreaker(), { wrapper })
    await result.current.mutateAsync()
    expect(apiClientMock.post).toHaveBeenCalledWith('/config/circuit-breaker/reset', {})
  })

  it('trips the circuit breaker with a custom reason', async () => {
    const data = { success: true, message: 'ok', previous_state: 'x', new_state: 'y' }
    apiClientMock.post.mockResolvedValue({ data })
    const { result } = renderHook(() => useTripCircuitBreaker(), { wrapper })
    await result.current.mutateAsync('because')
    expect(apiClientMock.post).toHaveBeenCalledWith('/config/circuit-breaker/trip', {
      reason: 'because',
    })
  })

  it('trips the circuit breaker with the default reason', async () => {
    const data = { success: true, message: 'ok', previous_state: 'x', new_state: 'y' }
    apiClientMock.post.mockResolvedValue({ data })
    const { result } = renderHook(() => useTripCircuitBreaker(), { wrapper })
    await result.current.mutateAsync()
    expect(apiClientMock.post).toHaveBeenCalledWith('/config/circuit-breaker/trip', {
      reason: 'Emergency kill switch activated',
    })
  })
})

describe('incidents', () => {
  it('fetches the dead letter queue', async () => {
    const data = { items: [], total: 0 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useDeadLetterQueue(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('fetches config audit with default pagination', async () => {
    const data = { items: [], total: 0 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useConfigAudit(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('fetches config audit with custom pagination', async () => {
    const data = { items: [], total: 0 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useConfigAudit({ limit: 5, offset: 10 }), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
    expect(apiClientMock.get).toHaveBeenCalledWith(
      '/incidents/config-audit?limit=5&offset=10',
      expect.anything()
    )
  })

  it('retries a dead letter item', async () => {
    const data = { success: true, message: 'ok', trade_uuid: 't1', retry_attempt: 1 }
    apiClientMock.post.mockResolvedValue({ data })
    await expect(retryDeadLetterItem('t1')).resolves.toBe(data)
    expect(apiClientMock.post).toHaveBeenCalledWith('/incidents/dead-letter/t1/retry')
  })

  it('retries a dead letter item with error wrapping', async () => {
    apiClientMock.post.mockRejectedValue(new Error('nope'))
    await expect(retryDeadLetterItem('t1')).rejects.toThrow('Retry failed: nope')
  })

  it('retries a dead letter item with a fallback message', async () => {
    apiClientMock.post.mockRejectedValue('string error')
    await expect(retryDeadLetterItem('t1')).rejects.toThrow('Retry failed. Please try again.')
  })
})

describe('metrics', () => {
  it('fetches performance metrics', async () => {
    const data = { pnl_24h: 1 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => usePerformanceMetrics(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('fetches strategy performance with default days', async () => {
    const data = { strategy: 'SHIELD', win_rate: 0.5 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useStrategyPerformance('SHIELD'), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
    expect(apiClientMock.get).toHaveBeenCalledWith(
      '/metrics/strategy',
      expect.objectContaining({ params: { strategy: 'SHIELD', days: '30' } })
    )
  })

  it('fetches cost metrics', async () => {
    const data = { avg_jito_tip_sol: 1 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useCostMetrics(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })
})

describe('balance & NAV', () => {
  it('computes balance, nav and loading state', async () => {
    apiClientMock.get.mockResolvedValueOnce({ data: { positions: [], total: 0, total_unrealized_pnl_sol: '5.5' } })
    apiClientMock.get.mockResolvedValueOnce({ data: { wallet_balance_sol: '100' } })
    const { result } = renderHook(() => useBalanceAndNAV(), { wrapper })
    await waitFor(() => expect(result.current.isLoading).toBe(false))
    expect(result.current.balance).toBe(100)
    expect(result.current.totalUnrealizedPnL).toBe(5.5)
    expect(result.current.nav).toBe(105.5)
    expect(result.current.isError).toBe(false)
  })

  it('falls back to zero when data is missing', async () => {
    apiClientMock.get.mockResolvedValue({ data: {} })
    const { result } = renderHook(() => useBalanceAndNAV(), { wrapper })
    await waitFor(() => expect(result.current.isLoading).toBe(false))
    expect(result.current.balance).toBe(0)
    expect(result.current.nav).toBe(0)
  })
})

describe('nav history', () => {
  it('fetches nav history with days', async () => {
    const data = { points: [], latest_nav_sol: null, latest_unrealized_pnl_sol: null }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useNavHistory(7), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
    expect(apiClientMock.get).toHaveBeenCalledWith(
      '/portfolio/nav-history',
      expect.objectContaining({ params: { days: 7 } })
    )
  })
})

describe('scout', () => {
  it('fetches scout status', async () => {
    const data = { status: 'idle', wqs_distribution: [], promotion_queue: [], rejection_queue: [] }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useScoutStatus(15000), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('rejects empty scout status payloads', async () => {
    apiClientMock.get.mockResolvedValue({ data: undefined })
    const { result } = renderHook(() => useScoutStatus(), { wrapper })
    await waitFor(() => expect(result.current.isError).toBe(true), { timeout: 15000 })
  }, 30000)

  it('fetches WQS distribution with a time range', async () => {
    const data = { distribution: [], average_score: 0, median_score: 0, total_wallets: 0 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useWQSDistribution('7d'), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
    expect(apiClientMock.get).toHaveBeenCalledWith(
      '/scout/wqs-distribution',
      expect.objectContaining({ params: { range: '7d' } })
    )
  })

  it('rejects null WQS distribution payloads', async () => {
    apiClientMock.get.mockResolvedValue({ data: null })
    const { result } = renderHook(() => useWQSDistribution(), { wrapper })
    await waitFor(() => expect(result.current.isError).toBe(true), { timeout: 15000 })
  }, 30000)

  it('fetches scout metrics', async () => {
    const data = { total_analyzed: 1 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useScoutMetrics('24h'), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('triggers a scout run', async () => {
    apiClientMock.post.mockResolvedValue({ data: { run_id: 'r1', scheduled_at: 'now' } })
    await expect(triggerScoutRun()).resolves.toEqual({ run_id: 'r1', scheduled_at: 'now' })
    expect(apiClientMock.post).toHaveBeenCalledWith('/scout/run', {})
  })

  it('throws when the scout run returns no data', async () => {
    apiClientMock.post.mockResolvedValue({ data: undefined })
    await expect(triggerScoutRun()).rejects.toThrow('Empty response from scout run')
  })

  it('wraps scout run errors', async () => {
    apiClientMock.post.mockRejectedValue(new Error('boom'))
    await expect(triggerScoutRun()).rejects.toThrow('boom')
  })

  it('fetches budget status', async () => {
    const data = { credits_used: 1 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useBudgetStatus(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('fetches cache stats', async () => {
    const data = { hit_rate: 0.5 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useCacheStats(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('fetches conviction allocation', async () => {
    const data = { total_wallets_analyzed: 10 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useConvictionAllocation(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })
})

describe('signals', () => {
  it('fetches signal quality with a time range', async () => {
    const data = { current_quality_score: 0.8, quality_distribution: [], average_quality_trend: [] }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useSignalQuality('24h'), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
    expect(apiClientMock.get).toHaveBeenCalledWith(
      '/signals/quality',
      expect.objectContaining({ params: { range: '24h' } })
    )
  })

  it('fetches signal sources', async () => {
    const data = { sources: [], total_signals: 0 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useSignalSources(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('fetches signal consensus', async () => {
    const data = { consensus_detection_rate: 0.4 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useSignalConsensus(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('fetches signal aggregation', async () => {
    const data = { total_aggregated_windows: 1 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useSignalAggregation('7d'), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('fetches signal clustering', async () => {
    const data = { total_clusters: 2 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useSignalClustering(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('surfaces signal query failures as errors', async () => {
    apiClientMock.get.mockRejectedValue(new Error('fail'))
    const { result } = renderHook(() => useSignalConsensus(), { wrapper })
    await waitFor(() => expect(result.current.isError).toBe(true), { timeout: 15000 })
  }, 30000)
})

describe('market', () => {
  it('fetches market regime with defaults for missing history', async () => {
    apiClientMock.get.mockResolvedValue({ data: { current_regime: 'bull', confidence: 1 } })
    const { result } = renderHook(() => useMarketRegime(), { wrapper })
    await waitFor(() => expect(result.current.data).toBeDefined())
    expect(result.current.data?.regime_history).toEqual([])
    expect(result.current.data?.performance_by_regime).toEqual([])
  })

  it('handles missing market regime data', async () => {
    apiClientMock.get.mockResolvedValue({ data: undefined })
    const { result } = renderHook(() => useMarketRegime(), { wrapper })
    await waitFor(() => expect(result.current.data).toBeDefined())
  })

  it('fetches market conditions with defaults', async () => {
    apiClientMock.get.mockResolvedValue({ data: { volatility_index: 10 } })
    const { result } = renderHook(() => useMarketConditions(), { wrapper })
    await waitFor(() => expect(result.current.data).toBeDefined())
    expect(result.current.data?.recommended_allocation).toEqual({ shield_percent: 0, spear_percent: 0 })
  })

  it('handles missing market conditions data', async () => {
    apiClientMock.get.mockResolvedValue({ data: undefined })
    const { result } = renderHook(() => useMarketConditions(), { wrapper })
    await waitFor(() => expect(result.current.data).toBeDefined())
  })
})

describe('risk', () => {
  it('fetches portfolio risk', async () => {
    const data = { portfolio_heat_percent: 50 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => usePortfolioRisk(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('fetches stop loss metrics for 24h and 90d ranges', async () => {
    const data = { activation_rate: 0.1 }
    apiClientMock.get.mockResolvedValue({ data })
    const a = renderHook(() => useStopLossMetrics('24h'), { wrapper })
    await waitFor(() => expect(a.result.current.data).toBe(data))
    const b = renderHook(() => useStopLossMetrics('90d'), { wrapper })
    await waitFor(() => expect(b.result.current.data).toBe(data))
  })

  it('fetches stop loss metrics for 7d and 30d ranges', async () => {
    const data = { activation_rate: 0.2 }
    apiClientMock.get.mockResolvedValue({ data })
    const a = renderHook(() => useStopLossMetrics('7d'), { wrapper })
    await waitFor(() => expect(a.result.current.data).toBe(data))
    const b = renderHook(() => useStopLossMetrics('30d'), { wrapper })
    await waitFor(() => expect(b.result.current.data).toBe(data))
  })

  it('fetches stop loss metrics for unknown ranges', async () => {
    const data = { activation_rate: 0.1 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useStopLossMetrics('random'), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
    expect(apiClientMock.get).toHaveBeenCalledWith(
      '/risk/stop-loss',
      expect.objectContaining({ params: { days: undefined } })
    )
  })

  it('fetches profit target metrics', async () => {
    const data = { hit_rate: 0.5 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useProfitTargetMetrics('7d'), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('fetches position size analysis', async () => {
    const data = { average_position_sol: 1 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => usePositionSizeAnalysis(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })
})

describe('reconciliation', () => {
  it('fetches reconciliation status', async () => {
    const data = { status: 'completed' }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useReconciliationStatus(15000), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('fetches reconciliation history', async () => {
    const data = { runs: [], total_runs: 0 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useReconciliationHistory(5), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('fetches reconciliation stats', async () => {
    const data = { total_reconciliations: 1 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useReconciliationStats('7d'), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('triggers reconciliation', async () => {
    apiClientMock.post.mockResolvedValue({ data: { run_id: 'r1', scheduled_at: 'now' } })
    const { result } = renderHook(() => useTriggerReconciliation(), { wrapper })
    await result.current.mutateAsync()
    expect(apiClientMock.post).toHaveBeenCalledWith('/reconciliation/trigger', {})
  })

  it('resolves a discrepancy', async () => {
    apiClientMock.post.mockResolvedValue({ data: { success: true } })
    const { result } = renderHook(() => useResolveDiscrepancy(), { wrapper })
    await result.current.mutateAsync({ id: 1, resolution: 'fixed' })
    expect(apiClientMock.post).toHaveBeenCalledWith('/reconciliation/discrepancies/1/resolve', {
      resolution: 'fixed',
    })
  })
})

describe('performance', () => {
  it('fetches trade latency with a range', async () => {
    const data = { p50: 10, histogram: [] }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useTradeLatency('24h'), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('transforms raw RPC latency payloads', async () => {
    apiClientMock.get.mockResolvedValue({
      data: {
        endpoints: [
          {
            endpoint: 'https://x',
            method: 'getHealth',
            avg_latency_ms: 12,
            p95_latency_ms: 20,
            p99_latency_ms: 30,
            error_rate_percent: 2,
            request_count: 100,
            success_rate_percent: 98,
          },
        ],
        overall_avg_ms: 12,
        overall_p95_ms: 20,
        overall_p99_ms: 30,
        error_rate_percent: 2,
        sample_size: 10,
      },
    })
    const { result } = renderHook(() => useRPCLatency(), { wrapper })
    await waitFor(() => expect(result.current.data).toBeDefined())
    expect(result.current.data?.endpoints[0].error_rate).toBe(2)
    expect(result.current.data?.endpoints[0].success_rate).toBe(0.98)
  })

  it('handles missing RPC latency data', async () => {
    apiClientMock.get.mockResolvedValue({ data: undefined })
    const { result } = renderHook(() => useRPCLatency(), { wrapper })
    await waitFor(() => expect(result.current.data).toBeDefined())
    expect(result.current.data?.overall_avg).toBe(0)
  })

  it('transforms raw database performance payloads', async () => {
    apiClientMock.get.mockResolvedValue({
      data: {
        query_latency: { avg_ms: 1, p95_ms: 2, p99_ms: 3, slow_queries_count: 4, total_queries_count: 5 },
        connection_pool: { active_connections: 1, idle_connections: 2, max_connections: 3, utilization_percent: 50 },
        cache_performance: { hit_rate: 80, miss_rate: 20, total_hits: 1, total_misses: 2, size: 3, max_size: 4 },
      },
    })
    const { result } = renderHook(() => useDatabasePerformance(), { wrapper })
    await waitFor(() => expect(result.current.data).toBeDefined())
    expect(result.current.data?.cache_performance.hit_rate).toBe(0.8)
  })

  it('handles missing database performance data', async () => {
    apiClientMock.get.mockResolvedValue({ data: undefined })
    const { result } = renderHook(() => useDatabasePerformance(), { wrapper })
    await waitFor(() => expect(result.current.data).toBeDefined())
    expect(result.current.data?.query_latency.slow_queries).toBe(0)
  })

  it('transforms raw request rate payloads', async () => {
    apiClientMock.get.mockResolvedValue({
      data: {
        current_rps: 5,
        peak_rps_24h: 10,
        avg_rps_1h: 3,
        overall_status: 'healthy',
        rate_limits: [
          {
            endpoint: '/api/v1/trades',
            metric_type: 'rps',
            current_rate: 4,
            limit: 10,
            utilization_percent: 40,
            window_seconds: 60,
            reset_at: '2025-01-01T00:00:00Z',
            status: 'ok',
          },
        ],
      },
    })
    const { result } = renderHook(() => useRequestRate(), { wrapper })
    await waitFor(() => expect(result.current.data).toBeDefined())
    expect(result.current.data?.rate_limits[0].remaining).toBe(6)
  })

  it('falls back to a computed reset time when reset_at is missing', async () => {
    apiClientMock.get.mockResolvedValue({
      data: {
        current_rps: 0,
        peak_rps_24h: 0,
        avg_rps_1h: 0,
        overall_status: 'healthy',
        rate_limits: [{ endpoint: '/api/v1/x', current_rate: 0, limit: 10, utilization_percent: 0, window_seconds: 60, status: 'ok' }],
      },
    })
    const { result } = renderHook(() => useRequestRate(), { wrapper })
    await waitFor(() => expect(result.current.data).toBeDefined())
    expect(result.current.data?.rate_limits[0].reset_at).toMatch(/T.*Z$/)
  })

  it('handles missing request rate data', async () => {
    apiClientMock.get.mockResolvedValue({ data: undefined })
    const { result } = renderHook(() => useRequestRate(), { wrapper })
    await waitFor(() => expect(result.current.data).toBeDefined())
    expect(result.current.data?.overall_status).toBe('healthy')
  })

  it('transforms raw cost metrics into cost analysis', async () => {
    apiClientMock.get.mockResolvedValue({
      data: {
        avg_jito_tip_sol: '0.001',
        avg_dex_fee_sol: '0.002',
        avg_slippage_cost_sol: '0.003',
        total_costs_30d_sol: '0.006',
        net_profit_30d_sol: '1.5',
        roi_percent: '10',
      },
    })
    const { result } = renderHook(() => useCostAnalysis('24h'), { wrapper })
    await waitFor(() => expect(result.current.data).toBeDefined())
    const d = result.current.data!
    expect(d.cost_by_type).toHaveLength(3)
    expect(d.total_costs).toBe(0.006)
  })

  it('handles missing cost metrics data', async () => {
    apiClientMock.get.mockResolvedValue({ data: undefined })
    const { result } = renderHook(() => useCostAnalysis(), { wrapper })
    await waitFor(() => expect(result.current.data).toBeDefined())
    expect(result.current.data?.total_costs).toBe(0)
  })
})

describe('operations', () => {
  it('fetches resource usage', async () => {
    const data = { memory: {}, disk: {}, cpu: {}, network: {}, timestamp: 'x' }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useResourceUsage(10000), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('fetches secret rotation', async () => {
    const data = { rotation_history: [] }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useSecretRotation(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('fetches rate limit status', async () => {
    const data = { endpoints: [], overall_status: 'healthy' }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useRateLimitStatus(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('fetches health check details', async () => {
    const data = { checks: [], overall_status: 'healthy' }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useHealthCheckDetails(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })
})

describe('consensus', () => {
  it('fetches consensus data', async () => {
    const data = { consensus_rate: 0.5 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useConsensus(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('fetches wallet clustering', async () => {
    const data = { clusters: [], total_wallets: 0 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useWalletClustering(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })

  it('fetches signal aggregation', async () => {
    const data = { window_start: 'x', window_end: 'y', total_signals: 1 }
    apiClientMock.get.mockResolvedValue({ data })
    const { result } = renderHook(() => useConsensusSignalAggregation(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(data))
  })
})
