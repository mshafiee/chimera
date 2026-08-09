import { describe, it, expect } from 'vitest'
import { render, screen } from '@testing-library/react'

vi.mock('recharts', async () => await import('../../../test-utils/rechartsMock'))
import { RegimeIndicator } from '../RegimeIndicator'
import { RegimeHistoryChart } from '../RegimeHistoryChart'
import { PerformanceByRegime } from '../PerformanceByRegime'
import * as marketBarrel from '../index'
import type { MarketRegimeResponse, PerformanceByRegime as Perf } from '../../api'

describe('market barrel', () => {
  it('re-exports all components', () => {
    expect(marketBarrel.RegimeIndicator).toBeTruthy()
    expect(marketBarrel.RegimeHistoryChart).toBeTruthy()
    expect(marketBarrel.PerformanceByRegime).toBeTruthy()
  })
})

describe('RegimeIndicator', () => {
  const base = {
    confidence: 0.85,
    last_regime_change: '2025-01-01T00:00:00Z',
    volatility_index: 0,
    trend_strength: 0,
    regime_history: [],
    performance_by_regime: [],
  }

  it.each(['bull', 'bear', 'neutral', 'volatile'] as const)('renders %s regime', (regime) => {
    render(<RegimeIndicator data={{ ...base, current_regime: regime } as MarketRegimeResponse} />)
    const labels: Record<string, string> = {
      bull: 'Bull Market',
      bear: 'Bear Market',
      neutral: 'Neutral Market',
      volatile: 'Volatile Market',
    }
    expect(screen.getByText(labels[regime])).toBeInTheDocument()
    expect(screen.getByText('Confidence: 85.0%')).toBeInTheDocument()
  })
})

describe('RegimeHistoryChart', () => {
  it('renders history points', () => {
    const { container } = render(
      <RegimeHistoryChart
        history={[
          { timestamp: '2025-01-01T00:00:00Z', regime: 'bull', volatility_index: 12.5 },
          { timestamp: '2025-01-02T00:00:00Z', regime: 'bear', volatility_index: 25.0 },
        ]}
      />
    )
    expect(container).toBeDefined()
  })

  it('renders empty history', () => {
    const { container } = render(<RegimeHistoryChart history={[]} />)
    expect(container).toBeDefined()
  })
})

describe('PerformanceByRegime', () => {
  const data: Perf[] = [
    { regime: 'bull', total_trades: 10, win_rate: 60, avg_return: 1.5, total_pnl: 15, sharpe_ratio: 1.2 },
    { regime: 'bear', total_trades: 5, win_rate: 40, avg_return: -0.5, total_pnl: -2.5, sharpe_ratio: 0.5 },
    { regime: 'neutral', total_trades: 3, win_rate: 50, avg_return: 0.2, total_pnl: 0.6, sharpe_ratio: -0.3 },
    { regime: 'volatile', total_trades: 7, win_rate: 55, avg_return: 0.8, total_pnl: 5.6, sharpe_ratio: 0.9 },
  ]

  it('renders rows with regime labels', () => {
    render(<PerformanceByRegime data={data} />)
    expect(screen.getByText('Bull')).toBeInTheDocument()
    expect(screen.getByText('Bear')).toBeInTheDocument()
    expect(screen.getByText('Neutral')).toBeInTheDocument()
    expect(screen.getByText('Volatile')).toBeInTheDocument()
    expect(screen.getByText('+$1.50')).toBeInTheDocument()
    expect(screen.getByText('$-0.50')).toBeInTheDocument()
  })
})
