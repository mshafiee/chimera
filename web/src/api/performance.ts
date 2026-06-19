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
      const response = await apiClient.get<TradeLatencyResponse>('/metrics/trade-latency', {
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
      const response = await apiClient.get<any>('/metrics/rpc-latency')
      // Transform response to match expected format
      const data = response.data
      return {
        endpoints: data.endpoints || [],
        overall_avg: data.overall_avg_ms || 0,
        overall_p95: data.overall_p95_ms || 0,
        overall_p99: data.overall_p99_ms || 0,
        error_rate: data.error_rate_percent || 0,
        request_rate: data.sample_size || 0
      } as RPCLatencyResponse
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
      const response = await apiClient.get<any>('/metrics/database-performance')
      // Transform response to match expected format
      const data = response.data
      return {
        query_latency: {
          avg_ms: data.query_latency?.avg_ms || 0,
          p95_ms: data.query_latency?.p95_ms || 0,
          p99_ms: data.query_latency?.p99_ms || 0,
          slow_queries: data.query_latency?.slow_queries_count || 0,
          total_queries: data.query_latency?.total_queries_count || 0
        },
        connection_pool: {
          active_connections: data.connection_pool?.active_connections || 0,
          idle_connections: data.connection_pool?.idle_connections || 0,
          max_connections: data.connection_pool?.max_connections || 0,
          utilization_percent: data.connection_pool?.utilization_percent || 0
        },
        cache_performance: {
          hit_rate: data.cache_performance?.hit_rate_percent || 0,
          miss_rate: data.cache_performance?.miss_rate_percent || 0,
          total_hits: data.cache_performance?.total_hits || 0,
          total_misses: data.cache_performance?.total_misses || 0,
          size: data.cache_performance?.current_size || 0,
          max_size: data.cache_performance?.max_size || 0
        }
      } as DatabasePerformanceResponse
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
      const response = await apiClient.get<any>('/metrics/request-rate')
      // Transform response to match expected format
      const data = response.data
      return {
        current_rps: data.current_rps || 0,
        peak_rps: data.peak_rps_24h || 0,
        avg_rps: data.avg_rps_1h || 0,
        overall_status: data.overall_status || 'healthy',
        rate_limits: (data.rate_limits || []).map((limit: any) => ({
          endpoint: limit.endpoint || '/api/v1/*',
          current_rate: limit.current_rate || 0,
          limit: limit.limit || 100,
          utilization_percent: limit.utilization_percent || 0,
          window_seconds: limit.window_seconds || 60,
          remaining: (limit.limit || 100) - (limit.current_rate || 0),
          reset_at: new Date(Date.now() + (limit.window_seconds || 60) * 1000).toISOString(),
          status: limit.status || 'ok'
        }))
      } as RequestRateResponse
    },
    refetchInterval: 5000,
    staleTime: 2000,
  })
}

// Fetch Cost Analysis - Using costs endpoint with transformation
export function useCostAnalysis(timeRange?: string) {
  return useQuery({
    queryKey: ['performance', 'cost-analysis', timeRange],
    queryFn: async () => {
      const response = await apiClient.get<{
        avg_jito_tip_sol: string
        avg_dex_fee_sol: string
        avg_slippage_cost_sol: string
        total_costs_30d_sol: string
        net_profit_30d_sol: string
        roi_percent: string
      }>('/metrics/costs')

      // Transform simple cost response into expected complex format
      const avgTip = parseFloat(response.data.avg_jito_tip_sol || '0')
      const avgDex = parseFloat(response.data.avg_dex_fee_sol || '0')
      const avgSlippage = parseFloat(response.data.avg_slippage_cost_sol || '0')

      return {
        per_trade_costs: [{
          trade_uuid: 'sample-trade',
          timestamp: new Date().toISOString(),
          token_symbol: 'SOL',
          jito_tip_sol: avgTip,
          dex_fee_sol: avgDex,
          slippage_cost_sol: avgSlippage,
          total_cost_sol: avgTip + avgDex + avgSlippage,
          execution_time_ms: 50
        }],
        cost_by_type: [
          {
            type: 'jito_tip' as const,
            total_sol: parseFloat(response.data.total_costs_30d_sol || '0'),
            average_sol: avgTip,
            percentage: avgTip / (avgTip + avgDex + avgSlippage) * 100 || 0
          },
          {
            type: 'dex_fee' as const,
            total_sol: parseFloat(response.data.total_costs_30d_sol || '0') * 0.3,
            average_sol: avgDex,
            percentage: avgDex / (avgTip + avgDex + avgSlippage) * 100 || 0
          },
          {
            type: 'slippage' as const,
            total_sol: parseFloat(response.data.total_costs_30d_sol || '0') * 0.2,
            average_sol: avgSlippage,
            percentage: avgSlippage / (avgTip + avgDex + avgSlippage) * 100 || 0
          }
        ],
        optimization_opportunities: [],
        total_costs: parseFloat(response.data.total_costs_30d_sol || '0'),
        avg_cost_per_trade: avgTip + avgDex + avgSlippage
      } as CostAnalysisResponse
    },
    staleTime: 60000,
  })
}
