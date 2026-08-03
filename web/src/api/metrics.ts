import { useQuery } from '@tanstack/react-query'
import { apiClient, getApiError } from './client'

export interface PerformanceMetrics {
  pnl_24h: number
  pnl_7d: number
  pnl_30d: number
  pnl_24h_change_percent: number | null
  pnl_7d_change_percent: number | null
  pnl_30d_change_percent: number | null
}

export interface StrategyPerformance {
  strategy: string
  win_rate: number
  avg_return: number
  trade_count: number
  total_pnl: number
}

export interface CostMetrics {
  avg_jito_tip_sol: number
  avg_dex_fee_sol: number
  avg_slippage_cost_sol: number
  total_costs_30d_sol: number
  net_profit_30d_sol: number
  roi_percent: number
}

const PERFORMANCE_ENDPOINT = '/metrics/performance'
const STRATEGY_ENDPOINT = '/metrics/strategy'
const COSTS_ENDPOINT = '/metrics/costs'

export function usePerformanceMetrics() {
  return useQuery({
    queryKey: ['metrics', 'performance'],
    queryFn: async ({ signal }) => {
      const { data } = await apiClient.get<PerformanceMetrics>(PERFORMANCE_ENDPOINT, { signal })
      return data
    },
    refetchInterval: 30000, // Refetch every 30 seconds
    meta: {
      onError: (error: unknown) => {
        console.error('[Metrics API] Failed to fetch performance metrics:', getApiError(error))
      },
    },
  })
}

export function useStrategyPerformance(strategy: 'SHIELD' | 'SPEAR', days: number = 30) {
  return useQuery({
    queryKey: ['metrics', 'strategy', strategy, days],
    queryFn: async ({ signal }) => {
      const { data } = await apiClient.get<StrategyPerformance>(
        STRATEGY_ENDPOINT,
        { params: { strategy, days: days.toString() }, signal }
      )
      return data
    },
    refetchInterval: 60000, // Refetch every minute
    meta: {
      onError: (error: unknown) => {
        console.error('[Metrics API] Failed to fetch strategy performance:', getApiError(error))
      },
    },
  })
}

export function useCostMetrics() {
  return useQuery({
    queryKey: ['metrics', 'costs'],
    queryFn: async ({ signal }) => {
      const { data } = await apiClient.get<CostMetrics>(COSTS_ENDPOINT, { signal })
      return data
    },
    refetchInterval: 60000, // Refetch every minute
    meta: {
      onError: (error: unknown) => {
        console.error('[Metrics API] Failed to fetch cost metrics:', getApiError(error))
      },
    },
  })
}
