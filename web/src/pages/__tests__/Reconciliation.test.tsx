import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { Reconciliation } from '../Reconciliation'

const apiMock = vi.hoisted(() => ({
  useReconciliationStatus: vi.fn(),
  useReconciliationHistory: vi.fn(),
  useTriggerReconciliation: vi.fn(),
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

const status = {
  last_reconciliation_at: '2025-01-01T00:00:00Z',
  next_reconciliation_at: '2025-01-02T00:00:00Z',
  status: 'completed',
  checked_count: 100,
  discrepancy_count: 2,
  unresolved_count: 1,
  duration_seconds: 12.5,
  recent_discrepancies: [
    { id: 1, trade_uuid: 'trade-1', type: 'missing_position', severity: 'high', description: 'missing', db_value: 'x', on_chain_value: null, detected_at: '2025-01-01T00:00:00Z', resolved: false, resolved_at: null },
  ],
}

const history = {
  runs: [{ id: 1, started_at: '2025-01-01T00:00:00Z', completed_at: null, status: 'completed', checked_count: 10, discrepancy_count: 0, unresolved_count: 0, duration_seconds: 5 }],
  total_runs: 1,
  success_rate: 1,
  avg_duration_seconds: 5,
}

function setup() {
  apiMock.useReconciliationStatus.mockReturnValue({
    data: status,
    isLoading: false,
    refetch: vi.fn(),
  })
  apiMock.useReconciliationHistory.mockReturnValue({ data: history, isLoading: false })
  apiMock.useTriggerReconciliation.mockReturnValue({
    mutateAsync: vi.fn().mockResolvedValue({ run_id: 'r1', scheduled_at: 'now' }),
    isPending: false,
  })
}

beforeEach(() => {
  vi.clearAllMocks()
  setup()
})

describe('Reconciliation', () => {
  it('renders the full page', () => {
    render(<Reconciliation />)
    expect(screen.getByText('Reconciliation')).toBeInTheDocument()
    expect(screen.getByText('Completed')).toBeInTheDocument()
    expect(screen.getAllByText('Checked').length).toBeGreaterThan(0)
    expect(screen.getAllByText('Discrepancies').length).toBeGreaterThan(0)
    expect(screen.getAllByText('Unresolved').length).toBeGreaterThan(0)
    expect(screen.getByText('1 Unresolved')).toBeInTheDocument()
    expect(screen.getByText('missing')).toBeInTheDocument()
    expect(screen.getByText('Reconciliation History')).toBeInTheDocument()
  })

  it('triggers a reconciliation run', async () => {
    render(<Reconciliation />)
    fireEvent.click(screen.getByRole('button', { name: /run reconciliation/i }))
    await waitFor(() => {
      expect(toastMock.info).toHaveBeenCalledWith('Triggering reconciliation...')
      expect(toastMock.success).toHaveBeenCalledWith('Reconciliation triggered successfully')
    })
  })

  it('shows an error when the trigger fails', async () => {
    apiMock.useTriggerReconciliation.mockReturnValue({
      mutateAsync: vi.fn().mockRejectedValue(new Error('no')),
      isPending: false,
    })
    render(<Reconciliation />)
    fireEvent.click(screen.getByRole('button', { name: /run reconciliation/i }))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Failed to trigger reconciliation')
    })
  })

  it('renders loading states', () => {
    apiMock.useReconciliationStatus.mockReturnValue({ data: undefined, isLoading: true, refetch: vi.fn() })
    apiMock.useReconciliationHistory.mockReturnValue({ data: undefined, isLoading: true })
    render(<Reconciliation />)
    expect(screen.getByText('Loading reconciliation status...')).toBeInTheDocument()
    expect(screen.getByText('Loading discrepancies...')).toBeInTheDocument()
    expect(screen.getByText('Loading history...')).toBeInTheDocument()
  })

  it('renders the in-sync empty state', () => {
    apiMock.useReconciliationStatus.mockReturnValue({
      data: { ...status, recent_discrepancies: [], discrepancy_count: 0, unresolved_count: 0 },
      isLoading: false,
      refetch: vi.fn(),
    })
    render(<Reconciliation />)
    expect(screen.getByText('No discrepancies found. System is in sync.')).toBeInTheDocument()
  })

  it('renders no history state', () => {
    apiMock.useReconciliationHistory.mockReturnValue({ data: undefined, isLoading: false })
    render(<Reconciliation />)
    expect(screen.getByText('No history available')).toBeInTheDocument()
  })

  it('changes the history limit', () => {
    render(<Reconciliation />)
    fireEvent.change(document.getElementById('reconciliation-history-limit') as HTMLSelectElement, {
      target: { value: '20' },
    })
    expect(apiMock.useReconciliationHistory).toHaveBeenCalled()
  })

  it('disables the trigger button while running', () => {
    apiMock.useReconciliationStatus.mockReturnValue({
      data: { ...status, status: 'running' },
      isLoading: false,
      refetch: vi.fn(),
    })
    apiMock.useTriggerReconciliation.mockReturnValue({
      mutateAsync: vi.fn(),
      isPending: true,
    })
    render(<Reconciliation />)
    expect(screen.getByRole('button', { name: /running\.\.\./i })).toBeDisabled()
  })
})
