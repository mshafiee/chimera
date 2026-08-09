import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { Risk } from '../Risk'

const apiMock = vi.hoisted(() => ({
  usePortfolioRisk: vi.fn(),
  useStopLossMetrics: vi.fn(),
  useProfitTargetMetrics: vi.fn(),
}))

vi.mock('../../api', () => apiMock)

const portfolioRisk = {
  portfolio_heat_percent: 85,
  heat_threshold: 80,
  heat_status: 'critical',
  concentration: {
    by_token: [{ token_address: 'tok-1', token_symbol: 'T1', position_count: 1, total_value_sol: 10, percentage: 30 }],
    by_sector: [{ sector: 'DeFi', position_count: 1, total_value_sol: 10, percentage: 30 }],
    max_concentration_percent: 30,
    hhi: 1500,
  },
  exposure: { total_exposure_sol: 10, long_exposure_sol: 10, short_exposure_sol: 0, net_exposure_sol: 10, max_drawdown_percent: 10, current_drawdown_percent: 5 },
  drawdown: { current_drawdown_percent: 5, max_drawdown_percent: 10, drawdown_duration_days: 3, recovery_percent: 50 },
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
  recent_hits: [],
}

function setup() {
  apiMock.usePortfolioRisk.mockReturnValue({ data: portfolioRisk, isLoading: false, error: null })
  apiMock.useStopLossMetrics.mockReturnValue({ data: stopLoss, isLoading: false })
  apiMock.useProfitTargetMetrics.mockReturnValue({ data: profitTargets, isLoading: false })
}

beforeEach(() => {
  vi.clearAllMocks()
  setup()
})

describe('Risk', () => {
  it('renders the full risk page with critical heat alert', () => {
    render(<Risk />)
    expect(screen.getByText('Risk Management')).toBeInTheDocument()
    expect(screen.getByText('Critical Risk Level')).toBeInTheDocument()
    expect(screen.getAllByText('Portfolio Heat').length).toBeGreaterThan(0)
    expect(screen.getByText('Concentration Analysis')).toBeInTheDocument()
    expect(screen.getByText('Current Drawdown')).toBeInTheDocument()
    expect(screen.getByText('Stop Loss Metrics')).toBeInTheDocument()
    expect(screen.getByText('Profit Target Metrics')).toBeInTheDocument()
    expect(screen.getByText('85.0%')).toBeInTheDocument()
  })

  it('renders loading states', () => {
    apiMock.usePortfolioRisk.mockReturnValue({ data: undefined, isLoading: true, error: null })
    apiMock.useStopLossMetrics.mockReturnValue({ data: undefined, isLoading: true })
    apiMock.useProfitTargetMetrics.mockReturnValue({ data: undefined, isLoading: true })
    render(<Risk />)
    expect(screen.getByText('Loading risk data...')).toBeInTheDocument()
    expect(screen.getByText('Loading stop loss data...')).toBeInTheDocument()
    expect(screen.getByText('Loading profit target data...')).toBeInTheDocument()
  })

  it('renders the error state', () => {
    apiMock.usePortfolioRisk.mockReturnValue({ data: undefined, isLoading: false, error: new Error('down') })
    render(<Risk />)
    expect(screen.getByText('Error loading risk data')).toBeInTheDocument()
  })

  it('renders the no-data notice and empty sections', () => {
    apiMock.usePortfolioRisk.mockReturnValue({ data: undefined, isLoading: false, error: null })
    apiMock.useStopLossMetrics.mockReturnValue({ data: undefined, isLoading: false })
    apiMock.useProfitTargetMetrics.mockReturnValue({ data: undefined, isLoading: false })
    render(<Risk />)
    expect(screen.getByText('No Portfolio Risk Data Available Yet')).toBeInTheDocument()
    expect(screen.getByText('No risk metrics available yet')).toBeInTheDocument()
    expect(screen.getByText('No stop loss metrics available yet')).toBeInTheDocument()
    expect(screen.getByText('No profit target metrics available yet')).toBeInTheDocument()
  })

  it('renders elevated heat alert', () => {
    apiMock.usePortfolioRisk.mockReturnValue({
      data: { ...portfolioRisk, heat_status: 'high', portfolio_heat_percent: 75 },
      isLoading: false,
      error: null,
    })
    render(<Risk />)
    expect(screen.getByText('Elevated Risk Level')).toBeInTheDocument()
  })

  it('changes the time range', () => {
    render(<Risk />)
    fireEvent.click(screen.getByText('7D'))
    expect(apiMock.useStopLossMetrics).toHaveBeenCalled()
  })

  it('tolerates proxy-wrapped portfolio risk data', () => {
    const proxy = new Proxy(
      {
        portfolio_heat_percent: 55,
        heat_threshold: 80,
        heat_status: 'normal',
        exposure: { total_exposure_sol: 1, long_exposure_sol: 1, short_exposure_sol: 0, net_exposure_sol: 1, max_drawdown_percent: 1, current_drawdown_percent: 0 },
        concentration: { by_token: [], by_sector: [], max_concentration_percent: 1, hhi: 1 },
      },
      {
        has(target, key) {
          if (key === 'portfolio_heat_percent') return Reflect.has(target, key)
          throw new Error('boom')
        },
      }
    ) as object
    apiMock.usePortfolioRisk.mockReturnValue({ data: proxy, isLoading: false, error: null })
    render(<Risk />)
    expect(screen.getByText('55.0%')).toBeInTheDocument()
    expect(screen.getAllByText('Portfolio Heat').length).toBeGreaterThan(0)
  })
})
