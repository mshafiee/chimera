import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { ScoutDashboard } from '../ScoutDashboard'

function findButton(text: string): HTMLElement {
  const matches = screen.getAllByRole('button').filter((b) => b.textContent?.includes(text))
  if (matches.length === 0) throw new Error(`Button containing "${text}" not found`)
  return matches[matches.length - 1]
}

const scoutApiMock = vi.hoisted(() => ({
  useBudgetStatus: vi.fn(),
  useCacheStats: vi.fn(),
  useConvictionAllocation: vi.fn(),
  triggerScoutRun: vi.fn(),
}))

vi.mock('../../api/scout', () => scoutApiMock)

const useWebSocketMock = vi.hoisted(() => vi.fn())
vi.mock('../../hooks/useWebSocket', () => ({ useWebSocket: useWebSocketMock }))

const toastMock = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  warning: vi.fn(),
  info: vi.fn(),
}))

vi.mock('sonner', () => ({ toast: toastMock }))

const budget = {
  credits_used: 5000,
  credits_remaining: 5000,
  total_monthly_credits: 10000,
  daily_target: 500,
  usage_percentage: 50,
  daily_usage_percentage: 50,
  alert_level: 'warning',
  forecast_24h: {
    horizon_hours: 24,
    projected_usage: 600,
    projected_remaining: 4400,
    confidence: 0.8,
    trend: 'increasing',
    recommendations: ['Reduce polling'],
  },
  optimization_suggestions: [
    { action_type: 'reduce', description: 'Lower cache TTL', expected_savings: 100, priority: 'high' },
  ],
}

const cache = {
  hit_rate: 80,
  miss_rate: 20,
  total_hits: 1000,
  total_misses: 250,
  total_entries: 500,
  max_size: 1000,
  activity_distribution: { very_high: 5, high: 10, medium: 20, low: 30, inactive: 15 },
  cache_efficiency: 85,
}

const conviction = {
  total_wallets_analyzed: 100,
  high_conviction_count: 12,
  budget_remaining: { high_conviction: 3000, emerging: 1000, reserve: 500 },
  wallets_analyzed: {
    very_high: { count: 5, credits_used: 500, average_wqs: 85, roi_score: 0.9 },
    high: { count: 10, credits_used: 1000, average_wqs: 75, roi_score: 0.8 },
    medium: { count: 20, credits_used: 2000, average_wqs: 60, roi_score: 0.5 },
    emerging: { count: 30, credits_used: 1000, average_wqs: 40, roi_score: 0.3 },
    low: { count: 35, credits_used: 500, average_wqs: 20, roi_score: 0.1 },
  },
  allocation_summary: {
    total_credits_allocated: 4500,
    high_conviction_percentage: 70,
    emerging_percentage: 20,
    average_credits_per_wallet: 45,
  },
}

function setup(loading = false) {
  scoutApiMock.useBudgetStatus.mockReturnValue({
    data: budget,
    isLoading: loading,
    error: null,
    refetch: vi.fn(),
  })
  scoutApiMock.useCacheStats.mockReturnValue({
    data: cache,
    isLoading: loading,
    error: null,
    refetch: vi.fn(),
  })
  scoutApiMock.useConvictionAllocation.mockReturnValue({
    data: conviction,
    isLoading: loading,
    error: null,
    refetch: vi.fn(),
  })
  scoutApiMock.triggerScoutRun.mockResolvedValue({ run_id: 'run-1', scheduled_at: 'now' })
  useWebSocketMock.mockReturnValue({ isConnected: true, isConnecting: false, connectionError: null })
}

beforeEach(() => {
  vi.clearAllMocks()
  setup()
})

describe('ScoutDashboard', () => {
  it('renders the loading state', () => {
    setup(true)
    render(<ScoutDashboard />)
    expect(screen.getByText('Loading Scout integration data...')).toBeInTheDocument()
  })

  it('renders the full dashboard', () => {
    render(<ScoutDashboard />)
    expect(screen.getByText('Scout Intelligence Dashboard')).toBeInTheDocument()
    expect(screen.getByText('Predictive Budget Manager')).toBeInTheDocument()
    expect(screen.getAllByText('5,000').length).toBeGreaterThan(0)
    expect(screen.getAllByText('50.0%').length).toBeGreaterThan(0)
    expect(screen.getByText('warning')).toBeInTheDocument()
    expect(screen.getByText('24-Hour Forecast')).toBeInTheDocument()
    expect(screen.getByText('Lower cache TTL')).toBeInTheDocument()
    expect(screen.getByText('Activity-Based Cache')).toBeInTheDocument()
    expect(screen.getAllByText('80.0%').length).toBeGreaterThan(0)
    expect(screen.getByText('High Conviction Allocator')).toBeInTheDocument()
    expect(screen.getByText('Very High (80+)')).toBeInTheDocument()
    expect(screen.getByText('Low (<30)')).toBeInTheDocument()
  })

  it('renders alert levels with different colors', () => {
    scoutApiMock.useBudgetStatus.mockReturnValue({
      data: { ...budget, alert_level: 'depleted' },
      isLoading: false,
      error: null,
      refetch: vi.fn(),
    })
    render(<ScoutDashboard />)
    expect(screen.getAllByText('depleted').length).toBeGreaterThan(0)

    scoutApiMock.useBudgetStatus.mockReturnValue({
      data: { ...budget, alert_level: 'critical' },
      isLoading: false,
      error: null,
      refetch: vi.fn(),
    })
    render(<ScoutDashboard />)
    expect(screen.getAllByText('critical').length).toBeGreaterThan(0)

    scoutApiMock.useBudgetStatus.mockReturnValue({
      data: { ...budget, alert_level: 'ok' },
      isLoading: false,
      error: null,
      refetch: vi.fn(),
    })
    render(<ScoutDashboard />)
    expect(screen.getAllByText('ok').length).toBeGreaterThan(0)
  })

  it('renders the error banner when data fails to load', () => {
    scoutApiMock.useBudgetStatus.mockReturnValue({
      data: undefined,
      isLoading: false,
      error: new Error('budget down'),
      refetch: vi.fn(),
    })
    render(<ScoutDashboard />)
    expect(screen.getByText('Data Loading Issues')).toBeInTheDocument()
  })

  it('runs scout analysis successfully', async () => {
    vi.useFakeTimers()
    const refetchBudget = vi.fn()
    const refetchCache = vi.fn()
    const refetchConviction = vi.fn()
    scoutApiMock.useBudgetStatus.mockReturnValue({ data: budget, isLoading: false, error: null, refetch: refetchBudget })
    scoutApiMock.useCacheStats.mockReturnValue({ data: cache, isLoading: false, error: null, refetch: refetchCache })
    scoutApiMock.useConvictionAllocation.mockReturnValue({ data: conviction, isLoading: false, error: null, refetch: refetchConviction })
    render(<ScoutDashboard />)
    fireEvent.click(findButton('Run Scout Analysis'))
    await vi.waitFor(() => {
      expect(scoutApiMock.triggerScoutRun).toHaveBeenCalled()
    })
    expect(toastMock.success).toHaveBeenCalledWith('Scout run initiated', {
      description: 'Run ID: run-1',
    })
    await vi.advanceTimersByTimeAsync(5000)
    expect(refetchBudget).toHaveBeenCalled()
    expect(refetchCache).toHaveBeenCalled()
    expect(refetchConviction).toHaveBeenCalled()
    vi.useRealTimers()
  })

  it('shows an error when the scout run fails', async () => {
    scoutApiMock.triggerScoutRun.mockRejectedValue(new Error('scout exploded'))
    render(<ScoutDashboard />)
    fireEvent.click(findButton('Run Scout Analysis'))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Failed to trigger Scout run', {
        description: 'scout exploded',
      })
    })
  })

  it('shows a fallback error description for unknown failures', async () => {
    scoutApiMock.triggerScoutRun.mockRejectedValue('string error')
    render(<ScoutDashboard />)
    fireEvent.click(findButton('Run Scout Analysis'))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Failed to trigger Scout run', {
        description: 'Unknown error',
      })
    })
  })

  it('refetches via the refresh buttons', () => {
    const refetchBudget = vi.fn()
    const refetchCache = vi.fn()
    const refetchConviction = vi.fn()
    scoutApiMock.useBudgetStatus.mockReturnValue({ data: budget, isLoading: false, error: null, refetch: refetchBudget })
    scoutApiMock.useCacheStats.mockReturnValue({ data: cache, isLoading: false, error: null, refetch: refetchCache })
    scoutApiMock.useConvictionAllocation.mockReturnValue({ data: conviction, isLoading: false, error: null, refetch: refetchConviction })
    render(<ScoutDashboard />)
    fireEvent.click(screen.getAllByRole('button')[1])
    expect(refetchBudget).toHaveBeenCalled()
    fireEvent.click(screen.getAllByRole('button')[2])
    expect(refetchCache).toHaveBeenCalled()
    fireEvent.click(screen.getAllByRole('button')[3])
    expect(refetchConviction).toHaveBeenCalled()
  })
})
