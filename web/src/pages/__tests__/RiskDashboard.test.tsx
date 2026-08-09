import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { RiskDashboard } from '../RiskDashboard'

vi.mock('recharts', async () => await import('../../test-utils/rechartsMock'))

const riskApiMock = vi.hoisted(() => ({
  usePortfolioRisk: vi.fn(),
  useStopLossMetrics: vi.fn(),
  useProfitTargetMetrics: vi.fn(),
  usePositionSizeAnalysis: vi.fn(),
}))

vi.mock('../../api/risk', () => riskApiMock)

const useWebSocketMock = vi.hoisted(() => vi.fn())
const useDashboardWebSocketMock = vi.hoisted(() => vi.fn())
const layoutMock = vi.hoisted(() => vi.fn())

vi.mock('../../hooks/useWebSocket', () => ({ useWebSocket: useWebSocketMock }))
vi.mock('../../hooks/useDashboardWebSocket', () => ({
  useDashboardWebSocket: useDashboardWebSocketMock,
  DASHBOARD_UPDATE_EVENT: 'dashboard:update',
}))
vi.mock('../../components/layout/Layout', () => ({ useLayoutContext: layoutMock }))

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

const portfolioRisk = {
  portfolio_heat_percent: 65,
  heat_threshold: 80,
  heat_status: 'elevated',
  concentration: {
    by_token: [{ token_address: 'tok-1', token_symbol: 'T1', position_count: 1, total_value_sol: 10, percentage: 25 }],
    by_sector: [{ sector: 'DeFi', position_count: 1, total_value_sol: 10, percentage: 25 }],
    max_concentration_percent: 25,
    hhi: 1200,
  },
  exposure: { total_exposure_sol: 10, long_exposure_sol: 10, short_exposure_sol: 0, net_exposure_sol: 10, max_drawdown_percent: 10, current_drawdown_percent: 3 },
  drawdown: { current_drawdown_percent: 3, max_drawdown_percent: 10, drawdown_duration_days: 2, recovery_percent: 70 },
  total_capital_sol: 100,
  wallet_balance_sol: 90,
}

const stopLoss = {
  activation_rate: 0.15,
  total_activations: 45,
  loss_prevented_sol: 12.5,
  average_loss_prevented_sol: 0.28,
  activations_by_strategy: [{ strategy: 'SHIELD', activations: 30, loss_prevented_sol: 8.5 }],
  recent_activations: [],
}

const profitTargets = {
  hit_rate: 0.68,
  total_hits: 34,
  total_targets: 50,
  trailing_stop_activations: 12,
  average_realized_gain_sol: 1.25,
  targets_by_strategy: [{ strategy: 'SHIELD', hit_rate: 0.72, total_hits: 18, average_gain_sol: 0.85 }],
  recent_hits: [{ timestamp: '2025-01-01T00:00:00Z', trade_uuid: 'h1', token_symbol: 'H1', target_level: 2, realized_gain_sol: 1.1, strategy: 'SHIELD' }],
}

const positionSize = {
  average_position_sol: 2.5,
  median_position_sol: 2,
  max_position_sol: 10,
  min_position_sol: 0.5,
  position_size_distribution: [
    { range: '0-1', count: 15, percentage: 15 },
    { range: '1-3', count: 50, percentage: 50 },
    { range: '3+', count: 35, percentage: 35 },
  ],
  kelly_criterion_usage: 0.75,
}

function setup() {
  riskApiMock.usePortfolioRisk.mockReturnValue({ data: portfolioRisk, isLoading: false, error: null })
  riskApiMock.useStopLossMetrics.mockReturnValue({ data: stopLoss, isLoading: false, error: null })
  riskApiMock.useProfitTargetMetrics.mockReturnValue({ data: profitTargets, isLoading: false, error: null })
  riskApiMock.usePositionSizeAnalysis.mockReturnValue({ data: positionSize, isLoading: false, error: null })
  useWebSocketMock.mockReturnValue({ isConnected: true, isConnecting: false, connectionError: null })
  useDashboardWebSocketMock.mockReturnValue({ refreshRiskData: vi.fn() })
  layoutMock.mockReturnValue({ setLastUpdate: vi.fn() })
}

beforeEach(() => {
  vi.clearAllMocks()
  setup()
})

describe('RiskDashboard', () => {
  it('renders the full dashboard', () => {
    render(<RiskDashboard />)
    expect(screen.getByText('Risk Analysis Dashboard')).toBeInTheDocument()
    expect(screen.getByText('Portfolio Heat')).toBeInTheDocument()
    expect(screen.getAllByText('Current Drawdown').length).toBeGreaterThan(0)
    expect(screen.getByText('Max Concentration')).toBeInTheDocument()
    expect(screen.getByText('Total Capital')).toBeInTheDocument()
    expect(screen.getByText('Position Sizing')).toBeInTheDocument()
    expect(screen.getByText('Kelly Criterion Usage')).toBeInTheDocument()
    expect(screen.getByText('Live')).toBeInTheDocument()
  })

  it('colors concentration by threshold', () => {
    riskApiMock.usePortfolioRisk.mockReturnValue({
      data: { ...portfolioRisk, concentration: { ...portfolioRisk.concentration, max_concentration_percent: 35 } },
      isLoading: false,
      error: null,
    })
    const { container } = render(<RiskDashboard />)
    expect(container.textContent).toContain('35.0%')

    riskApiMock.usePortfolioRisk.mockReturnValue({
      data: { ...portfolioRisk, concentration: { ...portfolioRisk.concentration, max_concentration_percent: 10 } },
      isLoading: false,
      error: null,
    })
    const { container: c2 } = render(<RiskDashboard />)
    expect(c2.textContent).toContain('10.0%')
  })

  it('renders the loading state', () => {
    riskApiMock.usePortfolioRisk.mockReturnValue({ data: undefined, isLoading: true, error: null })
    riskApiMock.useStopLossMetrics.mockReturnValue({ data: undefined, isLoading: true, error: null })
    riskApiMock.useProfitTargetMetrics.mockReturnValue({ data: undefined, isLoading: true, error: null })
    riskApiMock.usePositionSizeAnalysis.mockReturnValue({ data: undefined, isLoading: true, error: null })
    render(<RiskDashboard />)
    expect(screen.getByText('Loading risk analysis…')).toBeInTheDocument()
    expect(screen.getByText('Loading stop-loss & profit-target metrics…')).toBeInTheDocument()
    expect(screen.getByText('Loading position-size analysis…')).toBeInTheDocument()
  })

  it('renders the error state', () => {
    riskApiMock.usePortfolioRisk.mockReturnValue({ data: undefined, isLoading: false, error: new Error('down') })
    render(<RiskDashboard />)
    expect(screen.getByText('Failed to load risk data')).toBeInTheDocument()
    expect(screen.getByText(/Some data may be stale/)).toBeInTheDocument()
  })

  it('renders without position size data', () => {
    riskApiMock.usePositionSizeAnalysis.mockReturnValue({ data: undefined, isLoading: false, error: null })
    render(<RiskDashboard />)
    expect(screen.queryByText('Position Sizing')).not.toBeInTheDocument()
  })

  it('fires heat alert toasts via the websocket hook', () => {
    useDashboardWebSocketMock.mockImplementation(({ onHeatAlert }: { onHeatAlert: (d: { severity?: string; message?: string }) => void }) => {
      onHeatAlert({ severity: 'high', message: 'Heat high' })
      onHeatAlert({ severity: 'medium', message: 'Heat medium' })
      onHeatAlert({ severity: 'low', message: 'Heat low' })
      onHeatAlert({ message: 'Heat plain' })
      return { refreshRiskData: vi.fn() }
    })
    render(<RiskDashboard />)
    expect(toastMock.error).toHaveBeenCalledWith('Heat high', 10000)
    expect(toastMock.warning).toHaveBeenCalledWith('Heat medium')
    expect(toastMock.info).toHaveBeenCalledWith('Heat low')
    expect(toastMock.info).toHaveBeenCalledWith('Heat plain')
  })

  it('refreshes risk data', () => {
    const refreshRiskData = vi.fn()
    useDashboardWebSocketMock.mockReturnValue({ refreshRiskData })
    render(<RiskDashboard />)
    fireEvent.click(screen.getByRole('button', { name: /refresh/i }))
    expect(refreshRiskData).toHaveBeenCalled()
  })

  it('changes the time range', () => {
    render(<RiskDashboard />)
    fireEvent.click(screen.getByText('7D'))
    expect(riskApiMock.useStopLossMetrics).toHaveBeenCalled()
  })
})
