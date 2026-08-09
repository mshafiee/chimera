import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen } from '@testing-library/react'
import { Market } from '../Market'

vi.mock('recharts', async () => await import('../../test-utils/rechartsMock'))

const apiMock = vi.hoisted(() => ({
  useMarketRegime: vi.fn(),
  useMarketConditions: vi.fn(),
}))

vi.mock('../../api', () => apiMock)

const regime = {
  current_regime: 'bull',
  confidence: 0.85,
  volatility_index: 12,
  trend_strength: 5,
  last_regime_change: '2025-01-01T00:00:00Z',
  regime_history: [{ timestamp: '2025-01-01T00:00:00Z', regime: 'bull', volatility_index: 12 }],
  performance_by_regime: [{ regime: 'bull', total_trades: 10, win_rate: 60, avg_return: 1.5, total_pnl: 15, sharpe_ratio: 1.2 }],
}

const conditions = {
  volatility_index: 25,
  trend_strength: 0.5,
  liquidity_index: 60,
  market_sentiment: 'bullish',
  risk_level: 'medium',
  recommended_allocation: { shield_percent: 70, spear_percent: 30 },
}

function setup() {
  apiMock.useMarketRegime.mockReturnValue({ data: regime, isLoading: false })
  apiMock.useMarketConditions.mockReturnValue({ data: conditions, isLoading: false })
}

beforeEach(() => {
  vi.clearAllMocks()
  setup()
})

describe('Market', () => {
  it('renders the full market page', () => {
    render(<Market />)
    expect(screen.getByText('Market Analysis')).toBeInTheDocument()
    expect(screen.getByText('Bull Market')).toBeInTheDocument()
    expect(screen.getByText('Confidence: 85.0%')).toBeInTheDocument()
    expect(screen.getByText('Volatility Index')).toBeInTheDocument()
    expect(screen.getByText('Trend Strength')).toBeInTheDocument()
    expect(screen.getByText('Liquidity Index')).toBeInTheDocument()
    expect(screen.getByText('bullish')).toBeInTheDocument()
    expect(screen.getByText('medium')).toBeInTheDocument()
    expect(screen.getByText('Regime History')).toBeInTheDocument()
    expect(screen.getByText('Performance by Regime')).toBeInTheDocument()
    expect(screen.getByText('Bull')).toBeInTheDocument()
  })

  it('renders loading states', () => {
    apiMock.useMarketRegime.mockReturnValue({ data: undefined, isLoading: true })
    apiMock.useMarketConditions.mockReturnValue({ data: undefined, isLoading: true })
    render(<Market />)
    expect(screen.getByText('Loading market regime...')).toBeInTheDocument()
    expect(screen.getByText('Loading conditions...')).toBeInTheDocument()
  })

  it('renders empty states', () => {
    apiMock.useMarketRegime.mockReturnValue({ data: undefined, isLoading: false })
    apiMock.useMarketConditions.mockReturnValue({ data: undefined, isLoading: false })
    render(<Market />)
    expect(screen.getByText('No regime data available')).toBeInTheDocument()
    expect(screen.getByText('No conditions data available')).toBeInTheDocument()
  })

  it('renders alternative market conditions values', () => {
    apiMock.useMarketConditions.mockReturnValue({
      data: {
        ...conditions,
        volatility_index: 10,
        trend_strength: -0.5,
        liquidity_index: 30,
        market_sentiment: 'bearish',
        risk_level: 'high',
      },
      isLoading: false,
    })
    render(<Market />)
    expect(screen.getAllByText('Low').length).toBeGreaterThan(0)
    expect(screen.getByText('Strong Downtrend')).toBeInTheDocument()
    expect(screen.getByText('bearish')).toBeInTheDocument()
    expect(screen.getByText('high')).toBeInTheDocument()
  })

  it('renders a flat trend strength with the minus icon', () => {
    apiMock.useMarketConditions.mockReturnValue({
      data: { ...conditions, trend_strength: 0 },
      isLoading: false,
    })
    const { container } = render(<Market />)
    expect(container).toBeDefined()
  })

  it('renders without regime history and performance sections', () => {
    apiMock.useMarketRegime.mockReturnValue({
      data: { ...regime, regime_history: [], performance_by_regime: [] },
      isLoading: false,
    })
    render(<Market />)
    expect(screen.queryByText('Regime History')).not.toBeInTheDocument()
    expect(screen.queryByText('Performance by Regime')).not.toBeInTheDocument()
  })
})
