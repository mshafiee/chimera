import { useQuery } from '@tanstack/react-query'
import { toast } from 'sonner'
import { apiClient } from './client'

// Scout Status Response
export interface ScoutStatusResponse {
  last_run_at: string
  next_run_at: string | null
  wallets_analyzed: number
  analysis_duration_seconds: number
  status: 'running' | 'completed' | 'failed' | 'idle'
  wqs_distribution: WQSBucket[]
  promotion_queue: PromotionItem[]
  rejection_queue: RejectionItem[]
}

export interface WQSBucket {
  range: string // e.g., "0-20", "20-40", etc.
  count: number
  percentage: number
}

export interface PromotionItem {
  address: string
  wqs_score: number
  reason: string
  backtest_success: boolean
  validated_at: string
}

export interface RejectionItem {
  address: string
  wqs_score: number
  reason: string
  rejected_at: string
}

// WQS Distribution Response
export interface WQSDistributionResponse {
  distribution: WQSBucket[]
  average_score: number
  median_score: number
  total_wallets: number
}

// Scout Metrics Response
export interface ScoutMetricsResponse {
  total_analyzed: number
  rug_check_rejections: number
  backtest_success_rate: number
  validation_pass_rate: number
  avg_analysis_time_seconds: number
  liquidity_validation_rate: number
}

// Fetch Scout Status
export function useScoutStatus(refetchInterval?: number) {
  return useQuery({
    queryKey: ['scout', 'status'],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<ScoutStatusResponse>('/scout/status', { signal })
      return response.data
    },
    refetchInterval,
    staleTime: 5000,
    retry: 3,
    meta: {
      onError: (error: unknown) => {
        console.error('[Scout API] Failed to fetch status:', error)
        // Scout status is critical - show toast notification
        toast.error('Failed to load scout status. Please try again later.')
      },
    },
  })
}

// Fetch WQS Distribution
export function useWQSDistribution(timeRange?: string) {
  return useQuery({
    queryKey: ['scout', 'wqs-distribution', timeRange],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<WQSDistributionResponse>('/scout/wqs-distribution', {
        params: timeRange ? { range: timeRange } : undefined,
        signal,
      })
      return response.data
    },
    staleTime: 30000,
    retry: 3,
    meta: {
      onError: (error: unknown) => {
        console.error('[Scout API] Failed to fetch WQS distribution:', error)
        // WQS distribution is important - show toast notification
        toast.error('Failed to load WQS distribution. Please try again later.')
      },
    },
  })
}

// Fetch Scout Metrics
export function useScoutMetrics(timeRange?: string) {
  return useQuery({
    queryKey: ['scout', 'metrics', timeRange],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<ScoutMetricsResponse>('/scout/metrics', {
        params: timeRange ? { range: timeRange } : undefined,
        signal,
      })
      return response.data
    },
    staleTime: 60000,
    retry: 3,
    meta: {
      onError: (error: unknown) => {
        console.error('[Scout API] Failed to fetch metrics:', error)
        // Scout metrics are important - show toast notification
        toast.error('Failed to load scout metrics. Please try again later.')
      },
    },
  })
}

// Manual Scout Run Trigger
export async function triggerScoutRun(): Promise<{ run_id: string; scheduled_at: string }> {
  const response = await apiClient.post<{ run_id: string; scheduled_at: string }>('/scout/run', {})
  return response.data
}

// =============================================================================
// INTEGRATION FEATURE TYPES
// =============================================================================

// Budget Status Response
export interface BudgetStatusResponse {
  credits_used: number
  credits_remaining: number
  total_monthly_credits: number
  daily_target: number
  usage_percentage: number
  daily_usage_percentage: number
  alert_level: string
  forecast_24h: BudgetForecast
  optimization_suggestions: OptimizationSuggestion[]
}

export interface BudgetForecast {
  horizon_hours: number
  projected_usage: number
  projected_remaining: number
  confidence: number
  trend: string
  recommendations: string[]
}

export interface OptimizationSuggestion {
  action_type: string
  description: string
  expected_savings: number
  priority: string
}

// Cache Statistics Response
export interface CacheStatsResponse {
  hit_rate: number
  miss_rate: number
  total_hits: number
  total_misses: number
  total_entries: number
  max_size: number
  activity_distribution: ActivityDistribution
  cache_efficiency: number
}

export interface ActivityDistribution {
  very_high: number
  high: number
  medium: number
  low: number
  inactive: number
}

// Conviction Allocation Response
export interface ConvictionAllocationResponse {
  total_wallets_analyzed: number
  high_conviction_count: number
  budget_remaining: BudgetBreakdown
  wallets_analyzed: WalletAnalysisBreakdown
  allocation_summary: AllocationSummary
}

export interface BudgetBreakdown {
  high_conviction: number
  emerging: number
  reserve: number
}

export interface WalletAnalysisBreakdown {
  very_high: WalletLevelStats
  high: WalletLevelStats
  medium: WalletLevelStats
  emerging: WalletLevelStats
  low: WalletLevelStats
}

export interface WalletLevelStats {
  count: number
  credits_used: number
  average_wqs: number
  roi_score: number
}

export interface AllocationSummary {
  total_credits_allocated: number
  high_conviction_percentage: number
  emerging_percentage: number
  average_credits_per_wallet: number
}

// =============================================================================
// INTEGRATION FEATURE API HOOKS
// =============================================================================

// Fetch Budget Status
export function useBudgetStatus() {
  return useQuery({
    queryKey: ['scout', 'budget'],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<BudgetStatusResponse>('/scout/budget', { signal })
      return response.data
    },
    refetchInterval: 60000, // Refetch every minute
    retry: 2,
    meta: {
      onError: (error: unknown) => {
        console.error('[Scout API] Failed to fetch budget status:', error)
        toast.error('Failed to load budget status. Please try again later.')
      },
    },
  })
}

// Fetch Cache Statistics
export function useCacheStats() {
  return useQuery({
    queryKey: ['scout', 'cache'],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<CacheStatsResponse>('/scout/cache', { signal })
      return response.data
    },
    refetchInterval: 30000, // Refetch every 30 seconds
    retry: 2,
    meta: {
      onError: (error: unknown) => {
        console.error('[Scout API] Failed to fetch cache statistics:', error)
        toast.error('Failed to load cache statistics. Please try again later.')
      },
    },
  })
}

// Fetch Conviction Allocation
export function useConvictionAllocation() {
  return useQuery({
    queryKey: ['scout', 'conviction'],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<ConvictionAllocationResponse>('/scout/conviction', { signal })
      return response.data
    },
    refetchInterval: 120000, // Refetch every 2 minutes
    retry: 2,
    meta: {
      onError: (error: unknown) => {
        console.error('[Scout API] Failed to fetch conviction allocation:', error)
        toast.error('Failed to load conviction allocation. Please try again later.')
      },
    },
  })
}
