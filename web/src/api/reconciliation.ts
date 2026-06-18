import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiClient } from './client'

// Reconciliation Status Response
export interface ReconciliationStatusResponse {
  last_reconciliation_at: string | null
  next_reconciliation_at: string | null
  status: 'pending' | 'running' | 'completed' | 'failed'
  checked_count: number
  discrepancy_count: number
  unresolved_count: number
  duration_seconds: number | null
  recent_discrepancies: Discrepancy[]
}

export interface Discrepancy {
  id: number
  trade_uuid: string
  type: 'missing_position' | 'pnl_mismatch' | 'state_mismatch' | 'cost_mismatch'
  severity: 'low' | 'medium' | 'high' | 'critical'
  description: string
  db_value: string | null
  on_chain_value: string | null
  detected_at: string
  resolved: boolean
  resolved_at: string | null
}

// Reconciliation History
export interface ReconciliationHistoryResponse {
  runs: ReconciliationRun[]
  total_runs: number
  success_rate: number
  avg_duration_seconds: number
}

export interface ReconciliationRun {
  id: number
  started_at: string
  completed_at: string | null
  status: 'pending' | 'running' | 'completed' | 'failed'
  checked_count: number
  discrepancy_count: number
  unresolved_count: number
  duration_seconds: number | null
}

// Reconciliation Statistics
export interface ReconciliationStatsResponse {
  total_reconciliations: number
  successful_reconciliations: number
  failed_reconciliations: number
  total_checked: number
  total_discrepancies: number
  total_unresolved: number
  avg_discrepancies_per_run: number
  most_common_discrepancy_types: DiscrepancyTypeStats[]
}

export interface DiscrepancyTypeStats {
  type: string
  count: number
  percentage: number
}

// Mock data for when API is not available
const mockReconciliationStatus: ReconciliationStatusResponse = {
  last_reconciliation_at: null,
  next_reconciliation_at: null,
  status: 'pending',
  checked_count: 0,
  discrepancy_count: 0,
  unresolved_count: 0,
  duration_seconds: null,
  recent_discrepancies: []
}

const mockReconciliationHistory: ReconciliationHistoryResponse = {
  runs: [],
  total_runs: 0,
  success_rate: 0,
  avg_duration_seconds: 0
}

const mockReconciliationStats: ReconciliationStatsResponse = {
  total_reconciliations: 0,
  successful_reconciliations: 0,
  failed_reconciliations: 0,
  total_checked: 0,
  total_discrepancies: 0,
  total_unresolved: 0,
  avg_discrepancies_per_run: 0,
  most_common_discrepancy_types: []
}

// Fetch Reconciliation Status
export function useReconciliationStatus(refetchInterval?: number) {
  return useQuery({
    queryKey: ['reconciliation', 'status'],
    queryFn: async () => {
      try {
        const response = await apiClient.get<ReconciliationStatusResponse>('/reconciliation/status')
        return response.data
      } catch (error: any) {
        if (error.response?.status === 404) {
          console.warn('[Reconciliation API] Status endpoint not implemented, using mock data')
          return mockReconciliationStatus
        }
        throw error
      }
    },
    refetchInterval,
    staleTime: 5000,
  })
}

// Fetch Reconciliation History
export function useReconciliationHistory(limit?: number) {
  return useQuery({
    queryKey: ['reconciliation', 'history', limit],
    queryFn: async () => {
      try {
        const response = await apiClient.get<ReconciliationHistoryResponse>('/reconciliation/history', {
          params: limit ? { limit } : undefined,
        })
        return response.data
      } catch (error: any) {
        if (error.response?.status === 404) {
          console.warn('[Reconciliation API] History endpoint not implemented, using mock data')
          return mockReconciliationHistory
        }
        throw error
      }
    },
    staleTime: 60000,
  })
}

// Fetch Reconciliation Statistics
export function useReconciliationStats(timeRange?: string) {
  return useQuery({
    queryKey: ['reconciliation', 'stats', timeRange],
    queryFn: async () => {
      try {
        const response = await apiClient.get<ReconciliationStatsResponse>('/reconciliation/stats', {
          params: timeRange ? { range: timeRange } : undefined,
        })
        return response.data
      } catch (error: any) {
        if (error.response?.status === 404) {
          console.warn('[Reconciliation API] Stats endpoint not implemented, using mock data')
          return mockReconciliationStats
        }
        throw error
      }
    },
    staleTime: 300000,
  })
}

// Trigger Manual Reconciliation
export function useTriggerReconciliation() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async () => {
      const response = await apiClient.post<{ run_id: string; scheduled_at: string }>('/reconciliation/trigger')
      return response.data
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['reconciliation'] })
    },
  })
}

// Resolve Discrepancy
export function useResolveDiscrepancy() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async ({ id, resolution }: { id: number; resolution: string }) => {
      const response = await apiClient.post(`/api/v1/reconciliation/discrepancies/${id}/resolve`, { resolution })
      return response.data
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['reconciliation'] })
    },
  })
}
