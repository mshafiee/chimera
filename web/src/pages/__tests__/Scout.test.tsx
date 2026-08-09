import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { Scout } from '../Scout'

vi.mock('recharts', async () => await import('../../test-utils/rechartsMock'))

const apiMock = vi.hoisted(() => ({
  useScoutStatus: vi.fn(),
  useWQSDistribution: vi.fn(),
  useScoutMetrics: vi.fn(),
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

const status = {
  last_run_at: '2025-01-01T00:00:00Z',
  next_run_at: '2025-01-02T00:00:00Z',
  wallets_analyzed: 42,
  analysis_duration_seconds: 12.5,
  status: 'completed',
  wqs_distribution: [],
  promotion_queue: [
    { address: 'promo-address-1', wqs_score: 75.5, reason: 'strong ROI', backtest_success: true, validated_at: '2025-01-01T00:00:00Z' },
    { address: 'promo-address-2', wqs_score: 55.5, reason: 'decent', backtest_success: false, validated_at: '2025-01-01T00:00:00Z' },
  ],
  rejection_queue: [
    { address: 'rej-address-1', wqs_score: 20.5, reason: 'rug risk', rejected_at: '2025-01-01T00:00:00Z' },
  ],
}

function setup(overrides: Record<string, unknown> = {}) {
  apiMock.useScoutStatus.mockReturnValue({
    data: overrides.status ?? status,
    isLoading: false,
    refetch: vi.fn(),
  })
  apiMock.useWQSDistribution.mockReturnValue({
    data: {
      distribution: [{ range: '0-20', count: 5, percentage: 5 }],
      average_score: 60,
      median_score: 62,
      total_wallets: 100,
    },
    isLoading: false,
  })
  apiMock.useScoutMetrics.mockReturnValue({
    data: { total_analyzed: 100, rug_check_rejections: 10, backtest_success_rate: 75, validation_pass_rate: 80, avg_analysis_time_seconds: 3.5, liquidity_validation_rate: 90 },
    isLoading: false,
  })
  layoutMock.mockReturnValue({ setLastUpdate: vi.fn() })
}

beforeEach(() => {
  vi.clearAllMocks()
  setup()
})

describe('Scout', () => {
  it('renders the full scout page', () => {
    render(<Scout />)
    expect(screen.getByText('Scout Intelligence')).toBeInTheDocument()
    expect(screen.getByText('WQS Score Distribution')).toBeInTheDocument()
    expect(screen.getByText('Total Analyzed')).toBeInTheDocument()
    expect(screen.getByText('75.0%')).toBeInTheDocument()
    expect(screen.getByText(/Promotion Queue \(2\)/)).toBeInTheDocument()
    expect(screen.getByText('strong ROI')).toBeInTheDocument()
    expect(screen.getByText(/Rejection Queue \(1\)/)).toBeInTheDocument()
    expect(screen.getByText('rug risk')).toBeInTheDocument()
  })

  it('renders loading and empty states', () => {
    apiMock.useScoutStatus.mockReturnValue({ data: undefined, isLoading: true, refetch: vi.fn() })
    apiMock.useWQSDistribution.mockReturnValue({ data: undefined, isLoading: true })
    render(<Scout />)
    expect(screen.getByText('Loading WQS distribution...')).toBeInTheDocument()

    apiMock.useWQSDistribution.mockReturnValue({ data: undefined, isLoading: false })
    apiMock.useScoutStatus.mockReturnValue({ data: { ...status, promotion_queue: [], rejection_queue: [] }, isLoading: false, refetch: vi.fn() })
    render(<Scout />)
    expect(screen.getByText('No WQS data available')).toBeInTheDocument()
  })

  it('runs scout on button click', () => {
    render(<Scout />)
    fireEvent.click(screen.getByRole('button', { name: /run scout/i }))
    expect(toastMock.info).toHaveBeenCalledWith('Starting Scout run...')
    expect(toastMock.success).toHaveBeenCalledWith('Scout run initiated')
  })

  it('shows a toast when the scout run fails', () => {
    // The page's trigger call is commented out, so force a failure via refetch
    apiMock.useScoutStatus.mockReturnValue({
      data: undefined,
      isLoading: false,
      refetch: vi.fn(() => {
        throw new Error('refetch failed')
      }),
    })
    render(<Scout />)
    fireEvent.click(screen.getByRole('button', { name: /run scout/i }))
    expect(toastMock.error).toHaveBeenCalledWith('Failed to start Scout run')
  })

  it('disables the run button while running', () => {
    apiMock.useScoutStatus.mockReturnValue({
      data: { ...status, status: 'running' },
      isLoading: false,
      refetch: vi.fn(),
    })
    render(<Scout />)
    expect(screen.getByRole('button', { name: /run scout/i })).toBeDisabled()
  })

  it('changes the time range', () => {
    render(<Scout />)
    fireEvent.click(screen.getByText('30D'))
    expect(apiMock.useWQSDistribution).toHaveBeenCalled()
  })
})
