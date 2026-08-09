import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { Webhooks } from '../Webhooks'

const apiMock = vi.hoisted(() => ({
  useWebhookStats: vi.fn(),
  useWebhookAuditLog: vi.fn(),
  useBulkRegisterWebhooks: vi.fn(),
  useBulkCleanupWebhooks: vi.fn(),
  useReconcileWebhooks: vi.fn(),
  useHealthCheckWebhooks: vi.fn(),
}))

vi.mock('../../api', () => apiMock)

const toastMock = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  warning: vi.fn(),
  info: vi.fn(),
}))

vi.mock('../../components/ui/Toast', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../components/ui/Toast')>()
  return { ...actual, toast: toastMock }
})

const layoutMock = vi.hoisted(() => vi.fn())
vi.mock('../../components/layout/Layout', () => ({
  useLayoutContext: layoutMock,
}))

const stats = { total_webhooks: 10, active_webhooks: 8, stale_webhooks: 1, failed_registrations: 1 }

const auditLogs = [
  { id: 1, wallet_address: 'wallet-address-1', action: 'register', status: 'success', webhook_id: 'wh-1', details: null, error_message: null, duration_ms: 50, created_at: '2025-01-01T00:00:00Z' },
  { id: 2, wallet_address: 'wallet-address-2', action: 'delete', status: 'failed', webhook_id: null, details: null, error_message: 'auth', duration_ms: null, created_at: '2025-01-02T00:00:00Z' },
]

function setup() {
  apiMock.useWebhookStats.mockReturnValue({
    data: stats,
    isLoading: false,
    refetch: vi.fn(),
  })
  apiMock.useWebhookAuditLog.mockReturnValue({
    data: auditLogs,
    isLoading: false,
    refetch: vi.fn(),
  })
  apiMock.useBulkRegisterWebhooks.mockReturnValue({
    mutateAsync: vi.fn().mockResolvedValue({ total: 1, succeeded: 1, failed: 0, results: [] }),
    isPending: false,
  })
  apiMock.useBulkCleanupWebhooks.mockReturnValue({
    mutateAsync: vi.fn().mockResolvedValue({ total: 1, succeeded: 1, failed: 0, results: [] }),
    isPending: false,
  })
  apiMock.useReconcileWebhooks.mockReturnValue({
    mutateAsync: vi.fn().mockResolvedValue({ registered: 1, orphaned: 2, updated: 3, failed: 0, duration_ms: 10 }),
    isPending: false,
  })
  apiMock.useHealthCheckWebhooks.mockReturnValue({
    mutateAsync: vi.fn().mockResolvedValue({ total_checked: 5, healthy: 4, unhealthy: 1, cleaned_up: 1, duration_ms: 20 }),
    isPending: false,
  })
  layoutMock.mockReturnValue({ setLastUpdate: vi.fn() })
}

beforeEach(() => {
  vi.clearAllMocks()
  setup()
})

describe('Webhooks', () => {
  it('renders the full page', () => {
    render(<Webhooks />)
    expect(screen.getByText('Webhooks')).toBeInTheDocument()
    expect(screen.getByText('Total Webhooks')).toBeInTheDocument()
    expect(screen.getByText('Webhook Health')).toBeInTheDocument()
    expect(screen.getByText('Bulk Operations')).toBeInTheDocument()
    expect(screen.getByText('Audit Log')).toBeInTheDocument()
    expect(screen.getByText('register')).toBeInTheDocument()
    expect(screen.getByText('delete')).toBeInTheDocument()
  })

  it('runs reconciliation', async () => {
    render(<Webhooks />)
    fireEvent.click(screen.getByRole('button', { name: /reconcile/i }))
    await waitFor(() => {
      expect(toastMock.success).toHaveBeenCalledWith(
        'Reconciliation complete: 1 registered, 2 orphaned, 3 updated'
      )
    })
  })

  it('shows an error when reconciliation fails', async () => {
    apiMock.useReconcileWebhooks.mockReturnValue({
      mutateAsync: vi.fn().mockRejectedValue(new Error('reconcile failed')),
      isPending: false,
    })
    render(<Webhooks />)
    fireEvent.click(screen.getByRole('button', { name: /reconcile/i }))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('reconcile failed')
    })
  })

  it('runs a health check', async () => {
    render(<Webhooks />)
    fireEvent.click(screen.getByRole('button', { name: /run health check/i }))
    await waitFor(() => {
      expect(toastMock.success).toHaveBeenCalledWith(
        'Health check complete: 4 healthy, 1 unhealthy, 1 cleaned up'
      )
    })
  })

  it('shows an error when the health check fails', async () => {
    apiMock.useHealthCheckWebhooks.mockReturnValue({
      mutateAsync: vi.fn().mockRejectedValue(new Error('health failed')),
      isPending: false,
    })
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(<Webhooks />)
    fireEvent.click(screen.getByRole('button', { name: /run health check/i }))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('health failed')
    })
    errorSpy.mockRestore()
  })

  it('opens the bulk register modal and executes', async () => {
    render(<Webhooks />)
    fireEvent.click(screen.getByRole('button', { name: /bulk register/i }))
    expect(screen.getByText('Bulk Register Webhooks')).toBeInTheDocument()
    const textarea = screen.getByLabelText('Wallet Addresses') as HTMLTextAreaElement
    fireEvent.change(textarea, { target: { value: 'w1' } })
    fireEvent.click(screen.getByRole('button', { name: /start registration/i }))
    await waitFor(() => {
      expect(toastMock.success).toHaveBeenCalledWith('Webhook registration complete')
    })
  })

  it('opens the bulk cleanup modal and executes', async () => {
    render(<Webhooks />)
    fireEvent.click(screen.getByRole('button', { name: /bulk cleanup/i }))
    expect(screen.getByText('Bulk Cleanup Webhooks')).toBeInTheDocument()
    const textarea = screen.getByLabelText('Wallet Addresses') as HTMLTextAreaElement
    fireEvent.change(textarea, { target: { value: 'w1' } })
    fireEvent.click(screen.getByRole('button', { name: /start cleanup/i }))
    await waitFor(() => {
      expect(toastMock.success).toHaveBeenCalledWith('Webhook cleanup complete')
    })
  })

  it('filters the audit log by action and status', () => {
    render(<Webhooks />)
    const selects = document.querySelectorAll('select')
    fireEvent.change(selects[0], { target: { value: 'register' } })
    expect(apiMock.useWebhookAuditLog).toHaveBeenCalled()
    fireEvent.change(selects[1], { target: { value: 'failed' } })
    expect(apiMock.useWebhookAuditLog).toHaveBeenCalled()
  })

  it('refreshes stats via the refresh button', () => {
    const refetchStats = vi.fn()
    const refetchAudit = vi.fn()
    apiMock.useWebhookStats.mockReturnValue({ data: stats, isLoading: false, refetch: refetchStats })
    apiMock.useWebhookAuditLog.mockReturnValue({ data: auditLogs, isLoading: false, refetch: refetchAudit })
    render(<Webhooks />)
    fireEvent.click(screen.getAllByRole('button')[0])
    expect(refetchStats).toHaveBeenCalled()
    expect(refetchAudit).toHaveBeenCalled()
  })

  it('renders loading states', () => {
    apiMock.useWebhookStats.mockReturnValue({ data: undefined, isLoading: true, refetch: vi.fn() })
    apiMock.useWebhookAuditLog.mockReturnValue({ data: undefined, isLoading: true, refetch: vi.fn() })
    render(<Webhooks />)
    expect(screen.getByText('Loading audit log...')).toBeInTheDocument()
  })

  it('renders the empty audit log', () => {
    apiMock.useWebhookAuditLog.mockReturnValue({ data: [], isLoading: false, refetch: vi.fn() })
    render(<Webhooks />)
    expect(screen.getByText('No audit log entries found')).toBeInTheDocument()
  })
})
