import React from 'react'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { renderHook, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import type { ReactNode } from 'react'

const apiClientMock = vi.hoisted(() => ({
  get: vi.fn(),
  post: vi.fn(),
  put: vi.fn(),
}))

vi.mock('../client', () => ({
  apiClient: apiClientMock,
  getApiError: (error: unknown) =>
    error instanceof Error ? error.message : 'mock api error',
}))

vi.mock('sonner', () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
    info: vi.fn(),
  },
}))

import {
  useWebhookStats,
  useWebhookAuditLog,
  useBulkRegisterWebhooks,
  useBulkCleanupWebhooks,
  useReconcileWebhooks,
  useHealthCheckWebhooks,
  useToggleWebhook,
  useRetryWebhook,
} from '../webhooks'
import { useWalletMonitoringStates } from '../walletMonitoring'

let queryClient: QueryClient

function createWrapper() {
  queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  })
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  )
}

const wrapper = createWrapper()

beforeEach(() => {
  vi.resetAllMocks()
  queryClient.clear()
  apiClientMock.get.mockResolvedValue({ data: {} })
  apiClientMock.post.mockResolvedValue({ data: {}, status: 200 })
  apiClientMock.put.mockResolvedValue({ data: {} })
})

describe('webhooks', () => {
  it('fetches webhook stats', async () => {
    const payload = { total_webhooks: 1, active_webhooks: 1, stale_webhooks: 0, failed_registrations: 0 }
    apiClientMock.get.mockResolvedValue({ data: { success: true, data: payload } })
    const { result } = renderHook(() => useWebhookStats(30000), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(payload))
    expect(apiClientMock.get).toHaveBeenCalledWith('/monitoring/webhooks/stats', expect.anything())
  })

  it('throws when webhook stats fail', async () => {
    apiClientMock.get.mockResolvedValue({ data: { success: false, error: 'stats broke' } })
    const { result } = renderHook(() => useWebhookStats(), { wrapper })
    await waitFor(() => expect(result.current.isError).toBe(true))
    await waitFor(() => expect(result.current.error?.message).toBe('stats broke'))
  })

  it('fetches webhook audit log with filters', async () => {
    const payload = [{ id: 1, action: 'register', status: 'success' }]
    apiClientMock.get.mockResolvedValue({ data: { success: true, data: payload } })
    const { result } = renderHook(
      () => useWebhookAuditLog({ wallet_address: 'w1', action: 'register', status: 'success', limit: 10 }),
      { wrapper }
    )
    await waitFor(() => expect(result.current.data).toBe(payload))
    expect(apiClientMock.get).toHaveBeenCalledWith(
      '/monitoring/webhooks/audit',
      expect.objectContaining({
        params: { wallet_address: 'w1', action: 'register', status: 'success', limit: 10 },
      })
    )
  })

  it('fetches webhook audit log without filters', async () => {
    const payload = [{ id: 1, action: 'delete', status: 'failed' }]
    apiClientMock.get.mockResolvedValue({ data: { success: true, data: payload } })
    const { result } = renderHook(() => useWebhookAuditLog(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(payload))
  })

  it('throws when the audit log payload is missing', async () => {
    apiClientMock.get.mockResolvedValue({ data: { success: true, data: null } })
    const { result } = renderHook(() => useWebhookAuditLog(), { wrapper })
    await waitFor(() => expect(result.current.isError).toBe(true))
  })

  it('bulk registers webhooks', async () => {
    const payload = { total: 1, succeeded: 1, failed: 0, results: [] }
    apiClientMock.post.mockResolvedValue({ data: { success: true, data: payload } })
    const { result } = renderHook(() => useBulkRegisterWebhooks(), { wrapper })
    await expect(result.current.mutateAsync({ wallets: ['w1'] })).resolves.toBe(payload)
    expect(apiClientMock.post).toHaveBeenCalledWith('/monitoring/webhooks/bulk-register', {
      wallets: ['w1'],
    })
  })

  it('throws when bulk registration fails', async () => {
    apiClientMock.post.mockResolvedValue({ data: { success: false, error: 'reg failed' } })
    const { result } = renderHook(() => useBulkRegisterWebhooks(), { wrapper })
    await expect(result.current.mutateAsync({ wallets: [] })).rejects.toThrow('reg failed')
  })

  it('bulk cleans up webhooks', async () => {
    const payload = { total: 2, succeeded: 2, failed: 0, results: [] }
    apiClientMock.post.mockResolvedValue({ data: { success: true, data: payload } })
    const { result } = renderHook(() => useBulkCleanupWebhooks(), { wrapper })
    await expect(result.current.mutateAsync({ wallets: ['w1', 'w2'] })).resolves.toBe(payload)
    expect(apiClientMock.post).toHaveBeenCalledWith('/monitoring/webhooks/bulk-cleanup', {
      wallets: ['w1', 'w2'],
    })
  })

  it('reconciles webhooks', async () => {
    const payload = { registered: 1, orphaned: 0, updated: 0, failed: 0, duration_ms: 10 }
    apiClientMock.post.mockResolvedValue({ data: { success: true, data: payload } })
    const { result } = renderHook(() => useReconcileWebhooks(), { wrapper })
    await expect(result.current.mutateAsync()).resolves.toBe(payload)
    expect(apiClientMock.post).toHaveBeenCalledWith('/monitoring/webhooks/reconcile')
  })

  it('throws when reconciliation fails', async () => {
    apiClientMock.post.mockResolvedValue({ data: { success: false, error: 'reconcile failed' } })
    const { result } = renderHook(() => useReconcileWebhooks(), { wrapper })
    await expect(result.current.mutateAsync()).rejects.toThrow('reconcile failed')
  })

  it('runs webhook health checks', async () => {
    const payload = { total_checked: 1, healthy: 1, unhealthy: 0, cleaned_up: 0, duration_ms: 5 }
    apiClientMock.post.mockResolvedValue({ data: { success: true, data: payload } })
    const { result } = renderHook(() => useHealthCheckWebhooks(), { wrapper })
    await expect(result.current.mutateAsync()).resolves.toBe(payload)
    expect(apiClientMock.post).toHaveBeenCalledWith('/monitoring/webhooks/health-check')
  })

  it('throws when the health check fails', async () => {
    apiClientMock.post.mockResolvedValue({ data: { success: false, error: 'health failed' } })
    const { result } = renderHook(() => useHealthCheckWebhooks(), { wrapper })
    await expect(result.current.mutateAsync()).rejects.toThrow('health failed')
  })

  it('toggles a webhook', async () => {
    apiClientMock.post.mockResolvedValue({ status: 200 })
    const { result } = renderHook(() => useToggleWebhook(), { wrapper })
    await expect(result.current.mutateAsync({ walletAddress: 'w1', enabled: true })).resolves.toBe(true)
    expect(apiClientMock.post).toHaveBeenCalledWith('/monitoring/webhooks/w1/toggle', { enabled: true })
  })

  it('throws when the webhook toggle returns a non-2xx status', async () => {
    apiClientMock.post.mockResolvedValue({ status: 500 })
    const { result } = renderHook(() => useToggleWebhook(), { wrapper })
    await expect(result.current.mutateAsync({ walletAddress: 'w1', enabled: false })).rejects.toThrow(
      'Failed to toggle webhook'
    )
  })

  it('retries a webhook registration', async () => {
    apiClientMock.post.mockResolvedValue({ status: 200 })
    const { result } = renderHook(() => useRetryWebhook(), { wrapper })
    await expect(result.current.mutateAsync('w1')).resolves.toBe(true)
    expect(apiClientMock.post).toHaveBeenCalledWith('/monitoring/webhooks/w1/retry')
  })

  it('throws when the webhook retry returns a non-2xx status', async () => {
    apiClientMock.post.mockResolvedValue({ status: 400 })
    const { result } = renderHook(() => useRetryWebhook(), { wrapper })
    await expect(result.current.mutateAsync('w1')).rejects.toThrow(
      'Failed to retry webhook registration'
    )
  })
})

describe('wallet monitoring', () => {
  it('fetches wallet monitoring states', async () => {
    const payload = { wallet_states: [{ address: 'w1' }] }
    apiClientMock.get.mockResolvedValue({ data: payload })
    const { result } = renderHook(() => useWalletMonitoringStates(), { wrapper })
    await waitFor(() => expect(result.current.data).toBe(payload))
    expect(apiClientMock.get).toHaveBeenCalledWith('/monitoring/wallets/states', expect.anything())
  })

  it('throws on invalid wallet monitoring payloads', async () => {
    apiClientMock.get.mockResolvedValue({ data: { wallet_states: 'nope' } })
    const { result } = renderHook(() => useWalletMonitoringStates(), { wrapper })
    await waitFor(() => expect(result.current.isError).toBe(true), { timeout: 15000 })
  }, 30000)

  it('throws on missing wallet monitoring payloads', async () => {
    apiClientMock.get.mockResolvedValue({ data: undefined })
    const { result } = renderHook(() => useWalletMonitoringStates(), { wrapper })
    await waitFor(() => expect(result.current.isError).toBe(true), { timeout: 15000 })
  }, 30000)
})
