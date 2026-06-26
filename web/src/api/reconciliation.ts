import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
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

// Fetch Reconciliation Status
export function useReconciliationStatus(refetchInterval?: number) {
  return useQuery({
    queryKey: ['reconciliation', 'status'],
    queryFn: async ({ signal: _signal }) => {
      const response = await apiClient.get<ReconciliationStatusResponse>('/reconciliation/status')
      return response.data
    },
    refetchInterval,
    staleTime: 5000,
    retry: 1,
    meta: {
      onError: (error: unknown) => {
        console.error('[Reconciliation API] Failed to fetch status:', error)
        // Reconciliation requires authentication - handle 401 errors
        if (error && typeof error === 'object' && 'response' in error) {
          const err = error as { response?: { status?: number } }
          if (err.response?.status === 401) {
            toast.error('Authentication required for reconciliation data')
          }
        }
        // Reconciliation is optional - console only for other errors
      },
    },
  })
}

// Fetch Reconciliation History
export function useReconciliationHistory(limit?: number) {
  return useQuery({
    queryKey: ['reconciliation', 'history', limit],
    queryFn: async ({ signal: _signal }) => {
      const response = await apiClient.get<ReconciliationHistoryResponse>('/reconciliation/history', {
        params: limit ? { limit } : undefined,
      })
      return response.data
    },
    staleTime: 60000,
    retry: 1,
    meta: {
      onError: (error: unknown) => {
        console.error('[Reconciliation API] Failed to fetch history:', error)
        // Reconciliation requires authentication - handle 401 errors
        if (error && typeof error === 'object' && 'response' in error) {
          const err = error as { response?: { status?: number } }
          if (err.response?.status === 401) {
            toast.error('Authentication required for reconciliation history')
          }
        }
        // Reconciliation history is optional - console only for other errors
      },
    },
  })
}

// Fetch Reconciliation Statistics
export function useReconciliationStats(timeRange?: string) {
  return useQuery({
    queryKey: ['reconciliation', 'stats', timeRange],
    queryFn: async ({ signal: _signal }) => {
      const response = await apiClient.get<ReconciliationStatsResponse>('/reconciliation/stats', {
        params: timeRange ? { range: timeRange } : undefined,
      })
      return response.data
    },
    staleTime: 300000,
    retry: 1,
    meta: {
      onError: (error: unknown) => {
        console.error('[Reconciliation API] Failed to fetch stats:', error)
        // Reconciliation requires authentication - handle 401 errors
        if (error && typeof error === 'object' && 'response' in error) {
          const err = error as { response?: { status?: number } }
          if (err.response?.status === 401) {
            toast.error('Authentication required for reconciliation stats')
          }
        }
        // Reconciliation stats is optional - console only for other errors
      },
    },
  })
}

// Trigger Manual Reconciliation
export function useTriggerReconciliation() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async () => {
      const response = await apiClient.post<{ run_id: string; scheduled_at: string }>('/reconciliation/trigger', {})
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
