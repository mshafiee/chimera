import { useQuery } from '@tanstack/react-query'
import { toast } from 'sonner'
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

// Raw server payloads (serialized by the operator API)
interface RawRPCLatencyResponse {
  endpoints: RawRPCEndpointLatency[]
  overall_avg_ms: number
  overall_p95_ms: number
  overall_p99_ms: number
  error_rate_percent: number
  sample_size: number
}

interface RawRPCEndpointLatency {
  endpoint: string
  method: string
  avg_latency_ms: number
  p95_latency_ms: number
  p99_latency_ms: number
  error_rate_percent: number
  request_count: number
  success_rate_percent: number
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

interface RawDatabasePerformanceResponse {
  query_latency?: {
    avg_ms: number
    p95_ms: number
    p99_ms: number
    slow_queries_count: number
    total_queries_count: number
  }
  connection_pool?: {
    active_connections: number
    idle_connections: number
    max_connections: number
    utilization_percent: number
  }
  cache_performance?: {
    hit_rate: number
    miss_rate: number
    total_hits: number
    total_misses: number
    size: number
    max_size: number
  }
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

interface RawRequestRateResponse {
  current_rps: number
  peak_rps_24h: number
  avg_rps_1h: number
  overall_status: string
  rate_limits: RawRateLimitInfo[]
}

interface RawRateLimitInfo {
  endpoint: string
  metric_type: string
  current_rate: number
  limit: number
  utilization_percent: number
  window_seconds: number
  reset_at?: string
  status: string
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

interface RawCostMetricsResponse {
  avg_jito_tip_sol?: string
  avg_dex_fee_sol?: string
  avg_slippage_cost_sol?: string
  total_costs_30d_sol?: string
  net_profit_30d_sol?: string
  roi_percent?: string
}

const TRADE_LATENCY_ENDPOINT = '/metrics/trade-latency'
const RPC_LATENCY_ENDPOINT = '/metrics/rpc-latency'
const DATABASE_PERFORMANCE_ENDPOINT = '/metrics/database-performance'
const REQUEST_RATE_ENDPOINT = '/metrics/request-rate'
const COSTS_ENDPOINT = '/metrics/costs'

// Fetch Trade Latency
export function useTradeLatency(timeRange?: string) {
  return useQuery({
    queryKey: ['performance', 'trade-latency', timeRange],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<TradeLatencyResponse>(TRADE_LATENCY_ENDPOINT, {
        params: timeRange ? { range: timeRange } : undefined,
        signal,
      })
      return response.data
    },
    refetchInterval: 30000,
    staleTime: 15000,
    retry: 1,
    meta: {
      onError: (error: unknown) => {
        console.error('[Performance API] Failed to fetch trade latency:', error)
      },
    },
  })
}

// Fetch RPC Latency
export function useRPCLatency() {
  return useQuery({
    queryKey: ['performance', 'rpc-latency'],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<RawRPCLatencyResponse>(RPC_LATENCY_ENDPOINT, { signal })
      // Transform response to match expected format
      const data = response.data ?? {}
      return {
        endpoints: (data.endpoints || []).map((ep) => ({
          ...ep,
          error_rate: ep.error_rate_percent ?? 0,
          success_rate: (ep.success_rate_percent ?? 0) / 100,
        })),
        overall_avg: data.overall_avg_ms || 0,
        overall_p95: data.overall_p95_ms || 0,
        overall_p99: data.overall_p99_ms || 0,
        error_rate: data.error_rate_percent || 0,
      } as RPCLatencyResponse
    },
    refetchInterval: 10000,
    staleTime: 5000,
    retry: 3,
    meta: {
      onError: (error: unknown) => {
        console.error('[Performance API] Failed to fetch RPC latency:', error)
        toast.error('Failed to load RPC latency metrics. Please try again later.')
      },
    },
  })
}

// Fetch Database Performance
export function useDatabasePerformance() {
  return useQuery({
    queryKey: ['performance', 'database'],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<RawDatabasePerformanceResponse>(DATABASE_PERFORMANCE_ENDPOINT, { signal })
      // Transform response to match expected format
      const data = response.data ?? {}
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
          hit_rate: (data.cache_performance?.hit_rate ?? 0) / 100,
          miss_rate: (data.cache_performance?.miss_rate ?? 0) / 100,
          total_hits: data.cache_performance?.total_hits || 0,
          total_misses: data.cache_performance?.total_misses || 0,
          size: data.cache_performance?.size || 0,
          max_size: data.cache_performance?.max_size || 0
        }
      } as DatabasePerformanceResponse
    },
    refetchInterval: 30000,
    staleTime: 10000,
    retry: 3,
    meta: {
      onError: (error: unknown) => {
        console.error('[Performance API] Failed to fetch database performance:', error)
        toast.error('Failed to load database performance metrics. Please try again later.')
      },
    },
  })
}

// Fetch Request Rate
export function useRequestRate() {
  return useQuery({
    queryKey: ['performance', 'request-rate'],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<RawRequestRateResponse>(REQUEST_RATE_ENDPOINT, { signal })
      // Transform response to match expected format
      const data = response.data ?? {}
      return {
        current_rps: data.current_rps || 0,
        peak_rps: data.peak_rps_24h || 0,
        avg_rps: data.avg_rps_1h || 0,
        overall_status: data.overall_status || 'healthy',
        rate_limits: (data.rate_limits || []).map((limit) => ({
          endpoint: limit.endpoint || '/api/v1/*',
          current_rate: limit.current_rate || 0,
          limit: limit.limit || 100,
          utilization_percent: limit.utilization_percent || 0,
          window_seconds: limit.window_seconds || 60,
          remaining: Math.max(0, (limit.limit || 100) - (limit.current_rate || 0)),
          reset_at: limit.reset_at
            ? new Date(limit.reset_at).toISOString()
            : new Date(Date.now() + (limit.window_seconds || 60) * 1000).toISOString(),
          status: limit.status || 'ok'
        }))
      } as RequestRateResponse
    },
    refetchInterval: 5000,
    staleTime: 2000,
    retry: 1,
    meta: {
      onError: (error: unknown) => {
        console.error('[Performance API] Failed to fetch request rate:', error)
      },
    },
  })
}

// Fetch Cost Analysis - Using costs endpoint with transformation
export function useCostAnalysis(timeRange?: string) {
  return useQuery({
    queryKey: ['performance', 'cost-analysis', timeRange],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<RawCostMetricsResponse>(COSTS_ENDPOINT, {
        params: timeRange ? { range: timeRange } : undefined,
        signal,
      })

      // Transform simple cost response into expected complex format
      const data = response.data ?? {}
      const avgTip = parseFloat(data.avg_jito_tip_sol || '0')
      const avgDex = parseFloat(data.avg_dex_fee_sol || '0')
      const avgSlippage = parseFloat(data.avg_slippage_cost_sol || '0')
      const totalCosts = parseFloat(data.total_costs_30d_sol || '0')
      const avgTotal = avgTip + avgDex + avgSlippage
      const pct = (value: number) => (avgTotal > 0 ? (value / avgTotal) * 100 : 0)
      const shareOfTotal = (value: number) => (avgTotal > 0 ? (value / avgTotal) * totalCosts : 0)

      return {
        per_trade_costs: [],
        cost_by_type: [
          {
            type: 'jito_tip' as const,
            total_sol: shareOfTotal(avgTip),
            average_sol: avgTip,
            percentage: pct(avgTip),
          },
          {
            type: 'dex_fee' as const,
            total_sol: shareOfTotal(avgDex),
            average_sol: avgDex,
            percentage: pct(avgDex),
          },
          {
            type: 'slippage' as const,
            total_sol: shareOfTotal(avgSlippage),
            average_sol: avgSlippage,
            percentage: pct(avgSlippage),
          },
        ],
        optimization_opportunities: [],
        total_costs: totalCosts,
        avg_cost_per_trade: avgTotal,
      } as CostAnalysisResponse
    },
    staleTime: 60000,
    retry: 1,
    meta: {
      onError: (error: unknown) => {
        console.error('[Performance API] Failed to fetch cost analysis:', error)
      },
    },
  })
}
