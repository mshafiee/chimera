import { describe, it, expect, vi } from 'vitest'
import { render } from '@testing-library/react'
import * as chartBarrel from '../index'

vi.mock('recharts', async () => await import('../../../test-utils/rechartsMock'))

import { NavChart } from '../NavChart'
import { PnLChart } from '../PnLChart'
import { DrawdownChart } from '../DrawdownChart'
import { PortfolioHeatChart } from '../PortfolioHeatChart'
import { ConcentrationRiskChart } from '../ConcentrationRiskChart'
import { SignalConsensusChart } from '../SignalConsensusChart'
import { SignalQualityChart } from '../SignalQualityChart'
import { StopLossProfitChart } from '../StopLossProfitChart'

describe('charts barrel', () => {
  it('re-exports every chart', () => {
    expect(typeof chartBarrel.NavChart).toBe('function')
    expect(typeof chartBarrel.PnLChart).toBe('function')
    expect(typeof chartBarrel.PortfolioHeatChart).toBe('function')
    expect(typeof chartBarrel.ConcentrationRiskChart).toBe('function')
    expect(typeof chartBarrel.DrawdownChart).toBe('function')
    expect(typeof chartBarrel.StopLossProfitChart).toBe('function')
    expect(typeof chartBarrel.SignalQualityChart).toBe('function')
    expect(typeof chartBarrel.SignalConsensusChart).toBe('function')
  })
})

describe('NavChart', () => {
  const data = [
    { time: 'Jan 1', nav: 100, capital: 90 },
    { time: 'Jan 2', nav: 110, capital: 90 },
  ]

  it('renders with an explicit start capital', () => {
    const { container } = render(<NavChart data={data} startCapital={100} />)
    expect(container).toBeDefined()
  })

  it('renders without a start capital (falls back to first point)', () => {
    const { container } = render(<NavChart data={data} />)
    expect(container).toBeDefined()
  })

  it('renders negative trend', () => {
    const negative = [{ time: 'Jan 1', nav: 120, capital: 90 }, { time: 'Jan 2', nav: 110, capital: 90 }]
    const { container } = render(<NavChart data={negative} startCapital={100} />)
    expect(container).toBeDefined()
  })

  it('renders empty data without a reference line', () => {
    const { container } = render(<NavChart data={[]} />)
    expect(container).toBeDefined()
  })
})

describe('PnLChart', () => {
  it('renders positive data', () => {
    const { container } = render(<PnLChart data={[{ date: 'Jan 1', pnl: 5 }]} />)
    expect(container).toBeDefined()
  })

  it('renders negative data', () => {
    const { container } = render(<PnLChart data={[{ date: 'Jan 1', pnl: -5 }]} />)
    expect(container).toBeDefined()
  })

  it('renders empty data', () => {
    const { container } = render(<PnLChart data={[]} />)
    expect(container).toBeDefined()
  })
})

describe('DrawdownChart', () => {
  const base = {
    drawdownDurationDays: 10,
    recoveryPercent: 50,
  }

  it('renders with historical data in the minor range', () => {
    const { container } = render(
      <DrawdownChart
        {...base}
        currentDrawdownPercent={2}
        maxDrawdownPercent={20}
        historicalData={[{ timestamp: '2025-01-01', drawdown: 1, portfolio_value: 100 }]}
      />
    )
    expect(container).toBeDefined()
  })

  it('renders with generated data in the moderate range', () => {
    const { container } = render(
      <DrawdownChart {...base} currentDrawdownPercent={8} maxDrawdownPercent={20} />
    )
    expect(container).toBeDefined()
  })

  it('renders significant drawdown with elevated risk', () => {
    const { container } = render(
      <DrawdownChart
        {...base}
        currentDrawdownPercent={19}
        maxDrawdownPercent={20}
        historicalData={[]}
      />
    )
    expect(container).toBeDefined()
  })
})

describe('PortfolioHeatChart', () => {
  it.each(['normal', 'elevated', 'critical'] as const)('renders %s status', (status) => {
    const { container } = render(
      <PortfolioHeatChart heatPercentage={60} heatThreshold={80} heatStatus={status} />
    )
    expect(container).toBeDefined()
  })
})

describe('ConcentrationRiskChart', () => {
  const byToken = [
    { name: 'A Very Long Token Name Indeed', value: 10, percentage: 25 },
    { name: 'B', value: 5, percentage: 10 },
  ]
  const bySector = [
    { name: 'DeFi', value: 8, percentage: 20 },
    { name: 'Memes', value: 12, percentage: 30 },
  ]

  it('renders competitive HHI with warning alert', () => {
    const { container } = render(
      <ConcentrationRiskChart
        byToken={byToken}
        bySector={bySector}
        maxConcentrationPercent={25}
        hhi={1000}
      />
    )
    expect(container).toBeDefined()
  })

  it('renders moderate HHI', () => {
    const { container } = render(
      <ConcentrationRiskChart
        byToken={byToken}
        bySector={bySector}
        maxConcentrationPercent={15}
        hhi={2000}
      />
    )
    expect(container).toBeDefined()
  })

  it('renders high HHI', () => {
    const { container } = render(
      <ConcentrationRiskChart byToken={[]} bySector={[]} maxConcentrationPercent={5} hhi={3000} />
    )
    expect(container).toBeDefined()
  })
})

describe('SignalConsensusChart', () => {
  const consensusSignals = [
    {
      timestamp: '2025-01-01T00:00:00Z',
      token_address: 'tok1',
      token_symbol: 'T1',
      consensus_wallets: 5,
      total_wallets: 5,
      quality_score: 0.9,
    },
  ]
  const divergenceAlerts = [
    {
      timestamp: '2025-01-01T00:00:00Z',
      token_address: 'tok2',
      token_symbol: 'T2',
      divergence_score: 0.8,
      wallets_divergent: ['a', 'b', 'c'],
    },
    {
      timestamp: '2025-01-01T00:00:00Z',
      token_address: 'tok3',
      token_symbol: null,
      divergence_score: 0.4,
      wallets_divergent: ['a'],
    },
    {
      timestamp: '2025-01-01T00:00:00Z',
      token_address: 'tok4',
      token_symbol: 'T4',
      divergence_score: 0.5,
      wallets_divergent: ['a', 'b'],
    },
    {
      timestamp: '2025-01-01T00:00:00Z',
      token_address: 'tok5',
      token_symbol: 'T5',
      divergence_score: 0.6,
      wallets_divergent: ['a', 'b'],
    },
    {
      timestamp: '2025-01-01T00:00:00Z',
      token_address: 'tok6',
      token_symbol: 'T6',
      divergence_score: 0.5,
      wallets_divergent: ['a', 'b'],
    },
    {
      timestamp: '2025-01-01T00:00:00Z',
      token_address: 'tok7',
      token_symbol: 'T7',
      divergence_score: 0.5,
      wallets_divergent: ['a', 'b'],
    },
  ]

  it('renders with data and insights', () => {
    const { container } = render(
      <SignalConsensusChart
        consensusDetectionRate={0.8}
        averageClustering={0.3}
        divergenceAlerts={divergenceAlerts}
        consensusSignals={consensusSignals}
      />
    )
    expect(container).toBeDefined()
  })

  it('renders with empty lists', () => {
    const { container } = render(
      <SignalConsensusChart
        consensusDetectionRate={0.2}
        averageClustering={0.9}
        divergenceAlerts={[]}
        consensusSignals={[]}
      />
    )
    expect(container).toBeDefined()
  })

  it('renders the low agreement insight', () => {
    const { container } = render(
      <SignalConsensusChart
        consensusDetectionRate={0.2}
        averageClustering={0.4}
        divergenceAlerts={[]}
        consensusSignals={[]}
      />
    )
    expect(container).toBeDefined()
  })
})

describe('SignalQualityChart', () => {
  const qualityDistribution = [
    { range: '0-0.2', count: 5, percentage: 5 },
    { range: '0.2-0.4', count: 10, percentage: 10 },
    { range: '0.4-0.6', count: 20, percentage: 20 },
    { range: '0.6-0.8', count: 30, percentage: 30 },
    { range: '0.8-1.0', count: 35, percentage: 35 },
  ]

  it('renders excellent quality with trend data', () => {
    const { container } = render(
      <SignalQualityChart
        currentQualityScore={0.9}
        qualityDistribution={qualityDistribution}
        rejectionRate={0.1}
        totalSignals={100}
        acceptedSignals={90}
        rejectedSignals={10}
        averageQualityTrend={[
          { timestamp: '2025-01-01', average_score: 0.5 },
          { timestamp: '2025-01-02', average_score: 0.6 },
        ]}
      />
    )
    expect(container).toBeDefined()
  })

  it('renders poor quality with a high rejection rate and generated trend', () => {
    const { container } = render(
      <SignalQualityChart
        currentQualityScore={0.1}
        qualityDistribution={qualityDistribution}
        rejectionRate={0.5}
        totalSignals={100}
        acceptedSignals={50}
        rejectedSignals={50}
        averageQualityTrend={[]}
      />
    )
    expect(container).toBeDefined()
  })

  it('renders fair quality with empty distribution', () => {
    const { container } = render(
      <SignalQualityChart
        currentQualityScore={0.45}
        qualityDistribution={[]}
        rejectionRate={0}
        totalSignals={0}
        acceptedSignals={0}
        rejectedSignals={0}
        averageQualityTrend={[]}
      />
    )
    expect(container).toBeDefined()
  })
})

describe('StopLossProfitChart', () => {
  const activationsByStrategy = [
    { strategy: 'SHIELD', activations: 5, lossPrevented: 2.5, averageLoss: 0.5 },
  ]
  const targetsByStrategy = [
    { strategy: 'SHIELD', hitRate: 0.7, totalHits: 3, averageGain: 1.2 },
  ]

  it('renders full data with recent hits and insights', () => {
    const { container } = render(
      <StopLossProfitChart
        activationRate={0.2}
        totalActivations={5}
        lossPreventedSol={12}
        averageLossPreventedSol={0.5}
        activationsByStrategy={activationsByStrategy}
        hitRate={0.8}
        totalHits={3}
        totalTargets={5}
        trailingStopActivations={2}
        averageRealizedGainSol={1.1}
        targetsByStrategy={targetsByStrategy}
        recentHits={[
          { timestamp: '2025-01-01T00:00:00Z', token: 'T1', gain: 1.5 },
          { timestamp: '2025-01-02T00:00:00Z', token: 'T2', gain: 2.5 },
        ]}
      />
    )
    expect(container).toBeDefined()
  })

  it('renders without recent hits and low hit rate', () => {
    const { container } = render(
      <StopLossProfitChart
        activationRate={0.05}
        totalActivations={0}
        lossPreventedSol={0}
        averageLossPreventedSol={0}
        activationsByStrategy={[]}
        hitRate={0.2}
        totalHits={0}
        totalTargets={0}
        trailingStopActivations={0}
        averageRealizedGainSol={0}
        targetsByStrategy={[]}
      />
    )
    expect(container).toBeDefined()
  })
})
