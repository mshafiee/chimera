import React from 'react'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { renderHook, waitFor } from '@testing-library/react'
import { QueryCache, QueryClient, QueryClientProvider } from '@tanstack/react-query'
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

import { toast } from 'sonner'
import { usePositions, usePosition } from '../positions'
import { useConsensus, useWalletClustering, useSignalAggregation as useConsensusSignalAggregation } from '../consensus'
import { useMarketRegime, useMarketConditions } from '../market'
import { usePerformanceMetrics, useStrategyPerformance, useCostMetrics } from '../metrics'
import { useResourceUsage, useSecretRotation, useRateLimitStatus, useHealthCheckDetails } from '../operations'
import { useTradeLatency, useRPCLatency, useDatabasePerformance, useRequestRate, useCostAnalysis } from '../performance'
import { useReconciliationStatus, useReconciliationHistory, useReconciliationStats, useTriggerReconciliation, useResolveDiscrepancy } from '../reconciliation'
import { useScoutStatus, useWQSDistribution, useScoutMetrics, useBudgetStatus, useCacheStats, useConvictionAllocation } from '../scout'
import { useSignalQuality, useSignalSources, useSignalConsensus, useSignalAggregation, useSignalClustering } from '../signals'
import { useWalletMonitoringStates } from '../walletMonitoring'

let queryClient: QueryClient

function createWrapper() {
  queryClient = new QueryClient({
    queryCache: new QueryCache({
      onError: (error, query) => {
        const onError = query.meta?.onError
        if (typeof onError === 'function') {
          onError(error)
        }
      },
    }),
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
  vi.spyOn(console, 'error').mockImplementation(() => {})
  apiClientMock.get.mockRejectedValue(new Error('api down'))
})

afterEach(() => {
  vi.restoreAllMocks()
})

async function expectError(hook: () => void, timeout = 15000) {
  const { result } = renderHook(hook, { wrapper })
  await waitFor(() => expect(result.current.isError).toBe(true), { timeout })
}

describe('api error handlers', () => {
  it('positions: logs fetch failures', async () => {
    await expectError(() => usePositions())
    expect(console.error).toHaveBeenCalled()
  })

  it('position: logs single-position fetch failures', async () => {
    await expectError(() => usePosition('abc'))
    expect(console.error).toHaveBeenCalled()
  })

  it('consensus: logs consensus failures', async () => {
    await expectError(() => useConsensus())
    expect(console.error).toHaveBeenCalled()
  })

  it('consensus: logs clustering failures', async () => {
    await expectError(() => useWalletClustering())
    expect(console.error).toHaveBeenCalled()
  })

  it('consensus: logs aggregation failures', async () => {
    await expectError(() => useConsensusSignalAggregation())
    expect(console.error).toHaveBeenCalled()
  })

  it('market: logs regime failures', async () => {
    await expectError(() => useMarketRegime())
    expect(console.error).toHaveBeenCalled()
  })

  it('market: logs conditions failures', async () => {
    await expectError(() => useMarketConditions())
    expect(console.error).toHaveBeenCalled()
  })

  it('metrics: logs performance failures', async () => {
    await expectError(() => usePerformanceMetrics())
    expect(console.error).toHaveBeenCalled()
  })

  it('metrics: logs strategy performance failures', async () => {
    await expectError(() => useStrategyPerformance('SHIELD'))
    expect(console.error).toHaveBeenCalled()
  })

  it('metrics: logs cost metrics failures', async () => {
    await expectError(() => useCostMetrics())
    expect(console.error).toHaveBeenCalled()
  })

  it('operations: resource usage failures toast an error', async () => {
    await expectError(() => useResourceUsage(10000))
    expect(toast.error).toHaveBeenCalledWith('Failed to load resource usage. Please try again later.')
  }, 30000)

  it('operations: secret rotation failures are logged', async () => {
    await expectError(() => useSecretRotation())
    expect(console.error).toHaveBeenCalled()
  })

  it('operations: rate limit failures are logged', async () => {
    await expectError(() => useRateLimitStatus())
    expect(console.error).toHaveBeenCalled()
  })

  it('operations: health check failures toast an error', async () => {
    await expectError(() => useHealthCheckDetails())
    expect(toast.error).toHaveBeenCalledWith('Failed to load health check details. Please try again later.')
  }, 30000)

  it('performance: trade latency failures are logged', async () => {
    await expectError(() => useTradeLatency('24h'))
    expect(console.error).toHaveBeenCalled()
  })

  it('performance: RPC latency failures toast an error', async () => {
    await expectError(() => useRPCLatency())
    expect(toast.error).toHaveBeenCalledWith('Failed to load RPC latency metrics. Please try again later.')
  }, 30000)

  it('performance: database performance failures toast an error', async () => {
    await expectError(() => useDatabasePerformance())
    expect(toast.error).toHaveBeenCalledWith('Failed to load database performance metrics. Please try again later.')
  }, 30000)

  it('performance: request rate failures are logged', async () => {
    await expectError(() => useRequestRate())
    expect(console.error).toHaveBeenCalled()
  })

  it('performance: cost analysis failures are logged', async () => {
    await expectError(() => useCostAnalysis('24h'))
    expect(console.error).toHaveBeenCalled()
  })

  it('reconciliation: status failures are logged', async () => {
    await expectError(() => useReconciliationStatus(15000))
    expect(console.error).toHaveBeenCalled()
  })

  it('reconciliation: history failures are logged', async () => {
    await expectError(() => useReconciliationHistory())
    expect(console.error).toHaveBeenCalled()
  })

  it('reconciliation: stats failures are logged', async () => {
    await expectError(() => useReconciliationStats('7d'))
    expect(console.error).toHaveBeenCalled()
  })

  it('reconciliation: 401 status failures toast an auth message', async () => {
    apiClientMock.get.mockRejectedValue({ response: { status: 401 } })
    await expectError(() => useReconciliationStatus())
    expect(toast.error).toHaveBeenCalledWith('Authentication required for reconciliation data')
  })

  it('reconciliation: trigger failures toast the api error', async () => {
    apiClientMock.post.mockRejectedValue(new Error('trigger failed'))
    const { result } = renderHook(() => useTriggerReconciliation(), { wrapper })
    await expect(result.current.mutateAsync()).rejects.toThrow('trigger failed')
    expect(toast.error).toHaveBeenCalledWith('trigger failed')
  })

  it('reconciliation: resolve failures toast the api error', async () => {
    apiClientMock.post.mockRejectedValue(new Error('resolve failed'))
    const { result } = renderHook(() => useResolveDiscrepancy(), { wrapper })
    await expect(result.current.mutateAsync({ id: 1, resolution: 'x' })).rejects.toThrow('resolve failed')
    expect(toast.error).toHaveBeenCalledWith('resolve failed')
  })

  it('scout: status failures toast an error', async () => {
    await expectError(() => useScoutStatus())
    expect(toast.error).toHaveBeenCalledWith('Failed to load scout status. Please try again later.')
  }, 30000)

  it('scout: WQS distribution failures toast an error', async () => {
    await expectError(() => useWQSDistribution())
    expect(toast.error).toHaveBeenCalledWith('Failed to load WQS distribution. Please try again later.')
  }, 30000)

  it('scout: metrics failures toast an error', async () => {
    await expectError(() => useScoutMetrics())
    expect(toast.error).toHaveBeenCalledWith('Failed to load scout metrics. Please try again later.')
  }, 30000)

  it('scout: budget failures toast an error', async () => {
    await expectError(() => useBudgetStatus())
    expect(toast.error).toHaveBeenCalledWith('Failed to load budget status. Please try again later.')
  }, 30000)

  it('scout: cache failures toast an error', async () => {
    await expectError(() => useCacheStats())
    expect(toast.error).toHaveBeenCalledWith('Failed to load cache statistics. Please try again later.')
  }, 30000)

  it('scout: conviction failures toast an error', async () => {
    await expectError(() => useConvictionAllocation())
    expect(toast.error).toHaveBeenCalledWith('Failed to load conviction allocation. Please try again later.')
  }, 30000)

  it('signals: quality failures toast an error', async () => {
    await expectError(() => useSignalQuality('24h'))
    expect(toast.error).toHaveBeenCalledWith('Failed to load signal quality. Please try again later.')
  }, 30000)

  it('signals: sources failures are logged', async () => {
    await expectError(() => useSignalSources())
    expect(console.error).toHaveBeenCalled()
  })

  it('signals: consensus failures are logged', async () => {
    await expectError(() => useSignalConsensus())
    expect(console.error).toHaveBeenCalled()
  })

  it('signals: aggregation failures are logged', async () => {
    await expectError(() => useSignalAggregation())
    expect(console.error).toHaveBeenCalled()
  })

  it('signals: clustering failures are logged', async () => {
    await expectError(() => useSignalClustering())
    expect(console.error).toHaveBeenCalled()
  })

  it('wallet monitoring: failures toast an error', async () => {
    await expectError(() => useWalletMonitoringStates())
    expect(toast.error).toHaveBeenCalledWith('Failed to load wallet monitoring states. Please try again later.')
  }, 30000)
})
