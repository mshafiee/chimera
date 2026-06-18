import { useQuery } from '@tanstack/react-query'
import { apiClient } from './client'

// Trade Latency Response
export interface TradeLatencyResponse {
  p50: number
  p95: number
  p99: number
  max: number
  avg: number
  histogram: LatencyBucket[]
}

export interface LatencyBucket {
  range: string // e.g., "0-10ms", "10-50ms", etc.
  count: number
  percentage: number
}

// RPC Latency Response
export interface RPCLatencyResponse {
  endpoints: RPCEndpointLatency[]
  overall_avg: number
  overall_p95: number
  overall_p99: number
  error_rate: number
  request_rate: number
}

export interface RPCEndpointLatency {
  endpoint: string
  avg_latency_ms: number
  p95_latency_ms: number
  p99_latency_ms: number
  error_rate: number
  request_count: number
  success_rate: number
}

// Database Performance Response
export interface DatabasePerformanceResponse {
  query_latency: QueryLatencyStats
  connection_pool: ConnectionPoolStats
  cache_performance: CacheStats
}

export interface QueryLatencyStats {
  avg_ms: number
  p95_ms: number
  p99_ms: number
  slow_queries: number
  total_queries: number
}

export interface ConnectionPoolStats {
  active_connections: number
  idle_connections: number
  max_connections: number
  utilization_percent: number
}

export interface CacheStats {
  hit_rate: number
  miss_rate: number
  total_hits: number
  total_misses: number
  size: number
  max_size: number
}

// Request Rate Response
export interface RequestRateResponse {
  current_rps: number
  peak_rps: number
  avg_rps: number
  overall_status: 'healthy' | 'degraded' | 'throttled'
  rate_limits: RateLimitInfo[]
}

export interface RateLimitInfo {
  endpoint: string
  current_rate: number
  limit: number
  utilization_percent: number
  window_seconds: number
  remaining: number
  reset_at: string
  status: 'ok' | 'warning' | 'throttled'
}

// Cost Analysis Response (Enhanced)
export interface CostAnalysisResponse {
  per_trade_costs: CostByTrade[]
  cost_by_type: CostByType[]
  optimization_opportunities: OptimizationOpportunity[]
  total_costs: number
  avg_cost_per_trade: number
}

export interface CostByTrade {
  trade_uuid: string
  timestamp: string
  token_symbol: string | null
  jito_tip_sol: number
  dex_fee_sol: number
  slippage_cost_sol: number
  total_cost_sol: number
  execution_time_ms: number
}

export interface CostByType {
  type: 'jito_tip' | 'dex_fee' | 'slippage'
  total_sol: number
  average_sol: number
  percentage: number
}

export interface OptimizationOpportunity {
  type: string
  description: string
  potential_savings_sol: number
  current_value: number
  recommended_value: number
}

// Fetch Trade Latency
export function useTradeLatency(timeRange?: string) {
  return useQuery({
    queryKey: ['performance', 'trade-latency', timeRange],
    queryFn: async () => {
      const response = await apiClient.get<TradeLatencyResponse>('/api/v1/performance/latency', {
        params: timeRange ? { range: timeRange } : undefined,
      })
      return response.data
    },
    refetchInterval: 30000,
    staleTime: 15000,
  })
}

// Fetch RPC Latency
export function useRPCLatency() {
  return useQuery({
    queryKey: ['performance', 'rpc-latency'],
    queryFn: async () => {
      const response = await apiClient.get<RPCLatencyResponse>('/api/v1/performance/rpc')
      return response.data
    },
    refetchInterval: 10000,
    staleTime: 5000,
  })
}

// Fetch Database Performance
export function useDatabasePerformance() {
  return useQuery({
    queryKey: ['performance', 'database'],
    queryFn: async () => {
      const response = await apiClient.get<DatabasePerformanceResponse>('/api/v1/performance/database')
      return response.data
    },
    refetchInterval: 30000,
    staleTime: 10000,
  })
}

// Fetch Request Rate
export function useRequestRate() {
  return useQuery({
    queryKey: ['performance', 'request-rate'],
    queryFn: async () => {
      const response = await apiClient.get<RequestRateResponse>('/api/v1/performance/request-rate')
      return response.data
    },
    refetchInterval: 5000,
    staleTime: 2000,
  })
}

// Fetch Cost Analysis
export function useCostAnalysis(timeRange?: string) {
  return useQuery({
    queryKey: ['performance', 'cost-analysis', timeRange],
    queryFn: async () => {
      const response = await apiClient.get<CostAnalysisResponse>('/api/v1/performance/cost-analysis', {
        params: timeRange ? { range: timeRange } : undefined,
      })
      return response.data
    },
    staleTime: 60000,
  })
}
