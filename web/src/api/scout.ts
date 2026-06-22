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
    queryFn: async ({ signal: _signal }) => {
      const response = await apiClient.get<ScoutStatusResponse>('/scout/status')
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
    queryFn: async ({ signal: _signal }) => {
      const response = await apiClient.get<WQSDistributionResponse>('/scout/wqs-distribution', {
        params: timeRange ? { range: timeRange } : undefined,
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
    queryFn: async ({ signal: _signal }) => {
      const response = await apiClient.get<ScoutMetricsResponse>('/scout/metrics', {
        params: timeRange ? { range: timeRange } : undefined,
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
  const response = await apiClient.post<{ run_id: string; scheduled_at: string }>('/scout/run')
  return response.data
}
