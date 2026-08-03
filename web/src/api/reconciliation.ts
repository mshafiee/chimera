import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { apiClient, getApiError } from './client'

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
  type: Discrepancy['type']
  count: number
  percentage: number
}

const STATUS_ENDPOINT = '/reconciliation/status'
const HISTORY_ENDPOINT = '/reconciliation/history'
const STATS_ENDPOINT = '/reconciliation/stats'
const TRIGGER_ENDPOINT = '/reconciliation/trigger'

function handleReconciliationError(label: string, authMessage: string) {
  return (error: unknown) => {
    console.error(`[Reconciliation API] Failed to fetch ${label}:`, error)
    if (error && typeof error === 'object' && 'response' in error) {
      const err = error as { response?: { status?: number } }
      if (err.response?.status === 401) {
        toast.error(authMessage)
      }
    }
  }
}

// Fetch Reconciliation Status
export function useReconciliationStatus(refetchInterval?: number) {
  return useQuery({
    queryKey: ['reconciliation', 'status'],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<ReconciliationStatusResponse>(STATUS_ENDPOINT, { signal })
      return response.data
    },
    refetchInterval,
    staleTime: 5000,
    retry: 1,
    meta: {
      onError: handleReconciliationError('status', 'Authentication required for reconciliation data'),
    },
  })
}

// Fetch Reconciliation History
export function useReconciliationHistory(limit?: number) {
  return useQuery({
    queryKey: ['reconciliation', 'history', limit],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<ReconciliationHistoryResponse>(HISTORY_ENDPOINT, {
        params: limit !== undefined ? { limit } : undefined,
        signal,
      })
      return response.data
    },
    staleTime: 60000,
    retry: 1,
    meta: {
      onError: handleReconciliationError('history', 'Authentication required for reconciliation history'),
    },
  })
}

// Fetch Reconciliation Statistics
export function useReconciliationStats(timeRange?: string) {
  return useQuery({
    queryKey: ['reconciliation', 'stats', timeRange],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<ReconciliationStatsResponse>(STATS_ENDPOINT, {
        params: timeRange ? { range: timeRange } : undefined,
        signal,
      })
      return response.data
    },
    staleTime: 300000,
    retry: 1,
    meta: {
      onError: handleReconciliationError('stats', 'Authentication required for reconciliation stats'),
    },
  })
}

// Trigger Manual Reconciliation
export function useTriggerReconciliation() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async () => {
      const response = await apiClient.post<{ run_id: string; scheduled_at: string }>(TRIGGER_ENDPOINT, {})
      return response.data
    },
    onError: (error: unknown) => {
      toast.error(getApiError(error))
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
      const response = await apiClient.post<{ success: boolean }>(
        `/reconciliation/discrepancies/${id}/resolve`,
        { resolution }
      )
      return response.data
    },
    onError: (error: unknown) => {
      toast.error(getApiError(error))
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['reconciliation'] })
    },
  })
}
