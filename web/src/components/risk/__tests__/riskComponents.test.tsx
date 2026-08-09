import { describe, it, expect } from 'vitest'
import { render, screen } from '@testing-library/react'
import { PortfolioHeatGauge } from '../PortfolioHeatGauge'
import { ConcentrationMatrix } from '../ConcentrationMatrix'
import { StopLossAnalytics } from '../StopLossAnalytics'
import { ProfitTargetAnalytics } from '../ProfitTargetAnalytics'
import * as riskBarrel from '../index'
import type { PortfolioRiskResponse } from '../../api'

function makePortfolioRisk(heatStatus: 'normal' | 'elevated' | 'high' | 'critical'): PortfolioRiskResponse {
  return {
    portfolio_heat_percent: 65,
    heat_threshold: 80,
    heat_status: heatStatus,
    concentration: {
      by_token: [
        { token_address: 'tok-1', token_symbol: 'T1', position_count: 2, total_value_sol: 10, percentage: 25 },
        { token_address: 'tok-2', token_symbol: null, position_count: 1, total_value_sol: 5, percentage: 8 },
      ],
      by_sector: [
        { sector: 'DeFi', position_count: 2, total_value_sol: 8, percentage: 20 },
      ],
      max_concentration_percent: 25,
      hhi: 1250,
    },
    exposure: {
      total_exposure_sol: 15,
      long_exposure_sol: 15,
      short_exposure_sol: 0,
      net_exposure_sol: 15,
      max_drawdown_percent: 10,
      current_drawdown_percent: 3,
    },
    drawdown: {
      current_drawdown_percent: 3,
      max_drawdown_percent: 10,
      drawdown_duration_days: 5,
      recovery_percent: 70,
    },
    total_capital_sol: 100,
    wallet_balance_sol: 85,
  }
}

describe('risk barrel', () => {
  it('re-exports all components', () => {
    expect(riskBarrel.PortfolioHeatGauge).toBeTruthy()
    expect(riskBarrel.ConcentrationMatrix).toBeTruthy()
    expect(riskBarrel.StopLossAnalytics).toBeTruthy()
    expect(riskBarrel.ProfitTargetAnalytics).toBeTruthy()
  })
})

describe('PortfolioHeatGauge', () => {
  it.each(['normal', 'elevated', 'high', 'critical'] as const)('renders %s heat status', (status) => {
    render(<PortfolioHeatGauge data={makePortfolioRisk(status)} />)
    expect(screen.getByText('65.0%')).toBeInTheDocument()
    expect(screen.getByText(status.charAt(0).toUpperCase() + status.slice(1))).toBeInTheDocument()
    expect(screen.getByText('80%')).toBeInTheDocument()
    expect(screen.getByText('15.00 SOL')).toBeInTheDocument()
  })
})

describe('ConcentrationMatrix', () => {
  it('renders token and sector tables with metrics', () => {
    render(<ConcentrationMatrix data={makePortfolioRisk('normal').concentration} />)
    expect(screen.getByText('$T1')).toBeInTheDocument()
    expect(screen.getByText('$Unknown')).toBeInTheDocument()
    expect(screen.getByText('DeFi')).toBeInTheDocument()
    expect(screen.getAllByText('25.0%').length).toBeGreaterThan(0)
    expect(screen.getByText('1250.000')).toBeInTheDocument()
  })

  it('renders without sectors', () => {
    const data = makePortfolioRisk('normal').concentration
    render(<ConcentrationMatrix data={{ ...data, by_sector: [] }} />)
    expect(screen.queryByText('Concentration by Sector')).not.toBeInTheDocument()
  })
})

describe('StopLossAnalytics', () => {
  const data = {
    activation_rate: 0.15,
    total_activations: 45,
    loss_prevented_sol: 12.5,
    average_loss_prevented_sol: 0.28,
    activations_by_strategy: [
      { strategy: 'SHIELD' as const, activations: 30, loss_prevented_sol: 8.5 },
      { strategy: 'SPEAR' as const, activations: 15, loss_prevented_sol: 4.0 },
    ],
    recent_activations: [
      { timestamp: '2025-01-01T00:00:00Z', trade_uuid: 't1', token_symbol: 'T1', entry_price: 1.5, stop_price: 1.2, loss_prevented_sol: 0.3, strategy: 'SHIELD' as const },
      { timestamp: '2025-01-02T00:00:00Z', trade_uuid: 't2', token_symbol: null, entry_price: 2, stop_price: 1.8, loss_prevented_sol: 0.2, strategy: 'SPEAR' as const },
    ],
  }

  it('renders metrics, strategies and recent activations', () => {
    render(<StopLossAnalytics data={data} />)
    expect(screen.getByText('15.0%')).toBeInTheDocument()
    expect(screen.getByText('8.5000 SOL')).toBeInTheDocument()
    expect(screen.getByText('$T1')).toBeInTheDocument()
    expect(screen.getByText('$Unknown')).toBeInTheDocument()
  })

  it('renders without recent activations', () => {
    render(<StopLossAnalytics data={{ ...data, recent_activations: [] }} />)
    expect(screen.queryByText('Recent Activations')).not.toBeInTheDocument()
  })
})

describe('ProfitTargetAnalytics', () => {
  const data = {
    hit_rate: 0.68,
    total_hits: 34,
    total_targets: 50,
    trailing_stop_activations: 12,
    average_realized_gain_sol: 1.25,
    targets_by_strategy: [
      { strategy: 'SHIELD' as const, hit_rate: 0.72, total_hits: 18, average_gain_sol: 0.85 },
      { strategy: 'SPEAR' as const, hit_rate: 0.4, total_hits: 16, average_gain_sol: 1.68 },
    ],
    recent_hits: [
      { timestamp: '2025-01-01T00:00:00Z', trade_uuid: 'h1', token_symbol: 'H1', target_level: 2, realized_gain_sol: 1.1, strategy: 'SHIELD' as const },
    ],
  }

  it('renders metrics, strategies and recent hits', () => {
    render(<ProfitTargetAnalytics data={data} />)
    expect(screen.getByText('68.0%')).toBeInTheDocument()
    expect(screen.getByText('34 / 50')).toBeInTheDocument()
    expect(screen.getByText('$H1')).toBeInTheDocument()
    expect(screen.getByText('2x')).toBeInTheDocument()
  })

  it('renders without recent hits', () => {
    render(<ProfitTargetAnalytics data={{ ...data, recent_hits: [] }} />)
    expect(screen.queryByText('Recent Target Hits')).not.toBeInTheDocument()
  })
})
