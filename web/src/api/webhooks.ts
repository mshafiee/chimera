import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { apiClient } from './client'

// =============================================================================
// TYPES
// =============================================================================

// Webhook Statistics
export interface WebhookStats {
  total_webhooks: number
  active_webhooks: number
  stale_webhooks: number
  failed_registrations: number
}

// Webhook Audit Log Entry
export interface WebhookAuditLog {
  id: number
  wallet_address: string
  action: WebhookAction
  status: WebhookStatus
  webhook_id: string | null
  details: string | null
  error_message: string | null
  duration_ms: number | null
  created_at: string
}

export type WebhookAction = 'register' | 'update' | 'delete' | 'toggle' | 'health_check' | 'reconcile'
export type WebhookStatus = 'success' | 'failed' | 'pending' | 'retry'

// Bulk Operation Result
export interface BulkOperationResult {
  total: number
  succeeded: number
  failed: number
  results: BulkOperationItem[]
}

export interface BulkOperationItem {
  wallet_address: string
  success: boolean
  error?: string
}

// Reconciliation Result
export interface ReconciliationResult {
  registered: number
  orphaned: number
  updated: number
  failed: number
  duration_ms: number
}

// Health Check Result
export interface HealthCheckResult {
  total_checked: number
  healthy: number
  unhealthy: number
  cleaned_up: number
  duration_ms: number
}

// API Response Wrapper
export interface ApiResponse<T> {
  success: boolean
  data: T | null
  error: string | null
  message: string | null
}

// Request Types
export interface BulkRegisterRequest {
  wallets: string[]
  force_recreate?: boolean
}

export interface BulkCleanupRequest {
  wallets: string[]
}

export interface ToggleWebhookRequest {
  enabled: boolean
}

export interface WebhookAuditQuery {
  wallet_address?: string
  action?: WebhookAction
  status?: WebhookStatus
  limit?: number
}

// =============================================================================
// QUERY HOOKS
// =============================================================================

/**
 * Fetch webhook statistics
 */
export function useWebhookStats(refetchInterval: number = 30000) {
  return useQuery({
    queryKey: ['webhooks', 'stats'],
    queryFn: async () => {
      const response = await apiClient.get<ApiResponse<WebhookStats>>('/monitoring/webhooks/stats')
      if (response.data.success && response.data.data) {
        return response.data.data
      }
      throw new Error(response.data.error || 'Failed to fetch webhook statistics')
    },
    refetchInterval,
    staleTime: 10000,
  })
}

/**
 * Fetch webhook audit log with optional filtering
 */
export function useWebhookAuditLog(params: WebhookAuditQuery = {}) {
  return useQuery({
    queryKey: ['webhooks', 'audit', params],
    queryFn: async () => {
      const response = await apiClient.get<ApiResponse<WebhookAuditLog[]>>('/monitoring/webhooks/audit', {
        params: {
          ...(params.wallet_address && { wallet_address: params.wallet_address }),
          ...(params.action && { action: params.action }),
          ...(params.status && { status: params.status }),
          ...(params.limit && { limit: params.limit }),
        },
      })
      if (response.data.success && response.data.data) {
        return response.data.data
      }
      throw new Error(response.data.error || 'Failed to fetch audit log')
    },
    staleTime: 15000,
  })
}

// =============================================================================
// MUTATION HOOKS
// =============================================================================

/**
 * Bulk register webhooks for multiple wallets
 */
export function useBulkRegisterWebhooks() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async (request: BulkRegisterRequest) => {
      const response = await apiClient.post<ApiResponse<BulkOperationResult>>(
        '/monitoring/webhooks/bulk-register',
        request
      )
      if (response.data.success && response.data.data) {
        return response.data.data
      }
      throw new Error(response.data.error || 'Bulk webhook registration failed')
    },
    onSuccess: () => {
      // Invalidate related queries
      queryClient.invalidateQueries({ queryKey: ['webhooks'] })
    },
  })
}

/**
 * Bulk cleanup webhooks for multiple wallets
 */
export function useBulkCleanupWebhooks() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async (request: BulkCleanupRequest) => {
      const response = await apiClient.post<ApiResponse<BulkOperationResult>>(
        '/monitoring/webhooks/bulk-cleanup',
        request
      )
      if (response.data.success && response.data.data) {
        return response.data.data
      }
      throw new Error(response.data.error || 'Bulk webhook cleanup failed')
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['webhooks'] })
    },
  })
}

/**
 * Trigger manual webhook reconciliation
 */
export function useReconcileWebhooks() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async () => {
      const response = await apiClient.post<ApiResponse<ReconciliationResult>>(
        '/monitoring/webhooks/reconcile'
      )
      if (response.data.success && response.data.data) {
        return response.data.data
      }
      throw new Error(response.data.error || 'Webhook reconciliation failed')
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['webhooks'] })
    },
  })
}

/**
 * Trigger manual webhook health check
 */
export function useHealthCheckWebhooks() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async () => {
      const response = await apiClient.post<ApiResponse<HealthCheckResult>>(
        '/monitoring/webhooks/health-check'
      )
      if (response.data.success && response.data.data) {
        return response.data.data
      }
      throw new Error(response.data.error || 'Webhook health check failed')
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['webhooks'] })
    },
  })
}

/**
 * Toggle webhook enable/disable for a wallet
 */
export function useToggleWebhook() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async ({ walletAddress, enabled }: { walletAddress: string; enabled: boolean }) => {
      const response = await apiClient.post(
        `/monitoring/webhooks/${walletAddress}/toggle`,
        { enabled }
      )
      return response.status === 200
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['webhooks'] })
    },
  })
}

/**
 * Retry failed webhook registration for a wallet
 */
export function useRetryWebhook() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async (walletAddress: string) => {
      const response = await apiClient.post(
        `/monitoring/webhooks/${walletAddress}/retry`
      )
      return response.status === 200
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['webhooks'] })
    },
  })
}
