import { useQuery } from '@tanstack/react-query'
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

// Mock data for when API is not available
const mockScoutStatus: ScoutStatusResponse = {
  last_run_at: new Date(Date.now() - 3600000).toISOString(),
  next_run_at: new Date(Date.now() + 1800000).toISOString(),
  wallets_analyzed: 0,
  analysis_duration_seconds: 0,
  status: 'idle',
  wqs_distribution: [],
  promotion_queue: [],
  rejection_queue: []
}

const mockWQSDistribution: WQSDistributionResponse = {
  distribution: [
    { range: '0-20', count: 0, percentage: 0 },
    { range: '20-40', count: 0, percentage: 0 },
    { range: '40-60', count: 0, percentage: 0 },
    { range: '60-80', count: 0, percentage: 0 },
    { range: '80-100', count: 0, percentage: 0 }
  ],
  average_score: 0,
  median_score: 0,
  total_wallets: 0
}

const mockScoutMetrics: ScoutMetricsResponse = {
  total_analyzed: 0,
  rug_check_rejections: 0,
  backtest_success_rate: 0,
  validation_pass_rate: 0,
  avg_analysis_time_seconds: 0,
  liquidity_validation_rate: 0
}

// Fetch Scout Status
export function useScoutStatus(refetchInterval?: number) {
  return useQuery({
    queryKey: ['scout', 'status'],
    queryFn: async () => {
      try {
        const response = await apiClient.get<ScoutStatusResponse>('/scout/status')
        return response.data
      } catch (error: any) {
        // Return mock data if endpoint not available
        if (error.response?.status === 404) {
          console.warn('[Scout API] Status endpoint not implemented, using mock data')
          return mockScoutStatus
        }
        throw error
      }
    },
    refetchInterval,
    staleTime: 5000,
  })
}

// Fetch WQS Distribution
export function useWQSDistribution(timeRange?: string) {
  return useQuery({
    queryKey: ['scout', 'wqs-distribution', timeRange],
    queryFn: async () => {
      try {
        const response = await apiClient.get<WQSDistributionResponse>('/scout/wqs-distribution', {
          params: timeRange ? { range: timeRange } : undefined,
        })
        return response.data
      } catch (error: any) {
        // Return mock data if endpoint not available
        if (error.response?.status === 404) {
          console.warn('[Scout API] WQS distribution endpoint not implemented, using mock data')
          return mockWQSDistribution
        }
        throw error
      }
    },
    staleTime: 30000,
  })
}

// Fetch Scout Metrics
export function useScoutMetrics(timeRange?: string) {
  return useQuery({
    queryKey: ['scout', 'metrics', timeRange],
    queryFn: async () => {
      try {
        const response = await apiClient.get<ScoutMetricsResponse>('/scout/metrics', {
          params: timeRange ? { range: timeRange } : undefined,
        })
        return response.data
      } catch (error: any) {
        // Return mock data if endpoint not available
        if (error.response?.status === 404) {
          console.warn('[Scout API] Metrics endpoint not implemented, using mock data')
          return mockScoutMetrics
        }
        throw error
      }
    },
    staleTime: 60000,
  })
}

// Manual Scout Run Trigger
export async function triggerScoutRun(): Promise<{ run_id: string; scheduled_at: string }> {
  const response = await apiClient.post<{ run_id: string; scheduled_at: string }>('/scout/run')
  return response.data
}
