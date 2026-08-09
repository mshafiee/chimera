import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { Incidents } from '../Incidents'

const apiMock = vi.hoisted(() => ({
  useDeadLetterQueue: vi.fn(),
  useConfigAudit: vi.fn(),
  retryDeadLetterItem: vi.fn(),
}))

vi.mock('../../api', () => apiMock)

const toastMock = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  warning: vi.fn(),
  info: vi.fn(),
}))

vi.mock('sonner', () => ({ toast: toastMock }))

const incidents = [
  { id: 1, trade_uuid: 't1', payload: '{}', reason: 'MAX_RETRIES exceeded', error_details: 'boom', source_ip: '1.2.3.4', retry_count: 5, can_retry: true, received_at: '2025-01-01T00:00:00Z', processed_at: null },
  { id: 2, trade_uuid: 't2', payload: '{}', reason: 'QUEUE_FULL', error_details: null, source_ip: null, retry_count: 2, can_retry: false, received_at: '2025-01-02T00:00:00Z', processed_at: '2025-01-02T00:00:01Z' },
  { id: 3, trade_uuid: null, payload: '{}', reason: 'OTHER_REASON', error_details: 'x', source_ip: null, retry_count: 0, can_retry: true, received_at: '2025-01-03T00:00:00Z', processed_at: null },
]

const audits = [
  { id: 1, key: 'strategy.max_position_sol', old_value: '1', new_value: '2', changed_by: 'admin', change_reason: 'update', changed_at: '2025-01-01T00:00:00Z' },
  { id: 2, key: 'queue.capacity', old_value: null, new_value: '500', changed_by: 'op', change_reason: null, changed_at: '2025-01-02T00:00:00Z' },
]

function setup() {
  apiMock.useDeadLetterQueue.mockReturnValue({
    data: { items: incidents, total: incidents.length },
    isLoading: false,
    refetch: vi.fn(),
  })
  apiMock.useConfigAudit.mockReturnValue({ data: { items: audits, total: 2 }, isLoading: false })
  apiMock.retryDeadLetterItem.mockResolvedValue({ success: true, message: 'ok', trade_uuid: 't1', retry_attempt: 1 })
}

beforeEach(() => {
  vi.clearAllMocks()
  setup()
})

describe('Incidents', () => {
  it('renders the dead letter queue with severity badges', () => {
    render(<Incidents />)
    expect(screen.getByText('Critical')).toBeInTheDocument()
    expect(screen.getByText('Warning')).toBeInTheDocument()
    expect(screen.getByText('Info')).toBeInTheDocument()
    expect(screen.getByText('MAX_RETRIES exceeded')).toBeInTheDocument()
    expect(screen.getByText('QUEUE_FULL')).toBeInTheDocument()
    expect(screen.getByText('boom')).toBeInTheDocument()
  })

  it('filters by severity', () => {
    render(<Incidents />)
    fireEvent.click(screen.getByText('critical'))
    expect(screen.getByText('1 incident')).toBeInTheDocument()
    fireEvent.click(screen.getByText('warning'))
    expect(screen.getByText('1 incident')).toBeInTheDocument()
    fireEvent.click(screen.getByText('info'))
    expect(screen.getByText('1 incident')).toBeInTheDocument()
    fireEvent.click(screen.getByText('all'))
    expect(screen.getByText('3 incidents')).toBeInTheDocument()
  })

  it('retries a dead letter item', async () => {
    render(<Incidents />)
    fireEvent.click(screen.getAllByRole('button', { name: 'Retry' })[0])
    await waitFor(() => {
      expect(apiMock.retryDeadLetterItem).toHaveBeenCalledWith('t1')
    })
    expect(toastMock.success).toHaveBeenCalledWith('Trade queued for retry')
  })

  it('shows an error when the retry fails', async () => {
    apiMock.retryDeadLetterItem.mockRejectedValue(new Error('retry exploded'))
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(<Incidents />)
    fireEvent.click(screen.getAllByRole('button', { name: 'Retry' })[0])
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Failed to retry trade: retry exploded')
    })
    errorSpy.mockRestore()
  })

  it('renders the loading state', () => {
    apiMock.useDeadLetterQueue.mockReturnValue({ data: undefined, isLoading: true, refetch: vi.fn() })
    render(<Incidents />)
    expect(screen.getByText('Loading incidents...')).toBeInTheDocument()
  })

  it('renders the empty dead letter state', () => {
    apiMock.useDeadLetterQueue.mockReturnValue({ data: { items: [], total: 0 }, isLoading: false, refetch: vi.fn() })
    render(<Incidents />)
    expect(screen.getByText('The dead letter queue is empty')).toBeInTheDocument()
  })

  it('switches to the config audit tab', () => {
    render(<Incidents />)
    fireEvent.click(screen.getByText('Config Audit Log'))
    expect(screen.getByText('strategy.max_position_sol')).toBeInTheDocument()
    expect(screen.getByText('queue.capacity')).toBeInTheDocument()
    expect(screen.getByText('(none)')).toBeInTheDocument()
    expect(screen.getByText('update')).toBeInTheDocument()
  })

  it('renders audit loading state', () => {
    apiMock.useConfigAudit.mockReturnValue({ data: undefined, isLoading: true })
    render(<Incidents />)
    fireEvent.click(screen.getAllByText('Config Audit Log')[0])
    expect(screen.getByText('Loading audit log...')).toBeInTheDocument()
  })

  it('renders audit empty state', () => {
    apiMock.useConfigAudit.mockReturnValue({ data: { items: [], total: 0 }, isLoading: false })
    render(<Incidents />)
    fireEvent.click(screen.getAllByText('Config Audit Log')[0])
    expect(screen.getByText('No audit entries found')).toBeInTheDocument()
  })
})
