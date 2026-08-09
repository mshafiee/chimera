import { describe, it, expect } from 'vitest'
import { render, screen, fireEvent, act } from '@testing-library/react'
import { ConnectionStatus } from '../ConnectionStatus'
import { CostBreakdownChart } from '../CostBreakdownChart'
import { RealTimeAlerts } from '../RealTimeAlerts'
import { DASHBOARD_UPDATE_EVENT } from '../../../hooks/useDashboardWebSocket'
import { RPCLatencyMini } from '../RPCLatencyMini'
import { WalletAttribution } from '../WalletAttribution'
import * as dashboardBarrel from '../index'
import type { CostAnalysisResponse } from '../../api'

function dispatchAlert(type: string, data: Record<string, unknown>) {
  window.dispatchEvent(
    new CustomEvent(DASHBOARD_UPDATE_EVENT, { detail: { type, data } })
  )
}

describe('dashboard barrel', () => {
  it('re-exports all components', () => {
    expect(dashboardBarrel.CostBreakdownChart).toBeTruthy()
    expect(dashboardBarrel.WalletAttribution).toBeTruthy()
    expect(dashboardBarrel.RPCLatencyMini).toBeTruthy()
    expect(dashboardBarrel.RealTimeAlerts).toBeTruthy()
    expect(dashboardBarrel.ConnectionStatus).toBeTruthy()
  })
})

describe('ConnectionStatus (dashboard)', () => {
  it('renders each state', () => {
    const { container } = render(
      <ConnectionStatus isConnected={false} isConnecting={false} connectionError={null} />
    )
    expect(container.textContent).toContain('Disconnected')

    const { container: c2 } = render(
      <ConnectionStatus isConnected={false} isConnecting={true} connectionError={null} />
    )
    expect(c2.textContent).toContain('Connecting...')

    const { container: c3 } = render(
      <ConnectionStatus isConnected={false} isConnecting={false} connectionError="boom" />
    )
    expect(c3.textContent).toContain('Connection Error')

    const { container: c4 } = render(
      <ConnectionStatus isConnected={true} isConnecting={false} connectionError={null} />
    )
    expect(c4.textContent).toContain('Live')
  })
})

describe('CostBreakdownChart', () => {
  const data: CostAnalysisResponse = {
    per_trade_costs: [
      {
        trade_uuid: 't1',
        timestamp: '2025-01-01T00:00:00Z',
        token_symbol: 'SOL',
        jito_tip_sol: 0.001,
        dex_fee_sol: 0.002,
        slippage_cost_sol: 0.003,
        total_cost_sol: 0.006,
        execution_time_ms: 100,
      },
      {
        trade_uuid: 't2',
        timestamp: '2025-01-02T00:00:00Z',
        token_symbol: null,
        jito_tip_sol: 0.01,
        dex_fee_sol: 0.02,
        slippage_cost_sol: 0.03,
        total_cost_sol: 0.06,
        execution_time_ms: 200,
      },
    ],
    cost_by_type: [
      { type: 'jito_tip', total_sol: 0.011, average_sol: 0.0055, percentage: 16.7 },
      { type: 'dex_fee', total_sol: 0.022, average_sol: 0.011, percentage: 33.3 },
      { type: 'slippage', total_sol: 0.033, average_sol: 0.0165, percentage: 50 },
    ],
    optimization_opportunities: [],
    total_costs: 0.066,
    avg_cost_per_trade: 0.033,
  }

  it('renders cost breakdown and trade rows', () => {
    render(<CostBreakdownChart data={data} />)
    expect(screen.getByText('JITO TIP')).toBeInTheDocument()
    expect(screen.getByText('$SOL')).toBeInTheDocument()
    expect(screen.getByText('$Unknown')).toBeInTheDocument()
  })
})

describe('RealTimeAlerts', () => {
  it('renders nothing without alerts', () => {
    const { container } = render(<RealTimeAlerts />)
    expect(container.firstChild).toBeNull()
  })

  it('adds alerts from dashboard update events', () => {
    render(<RealTimeAlerts maxAlerts={2} />)
    act(() => {
      dispatchAlert('risk_update', { severity: 'high', message: 'Risk rising', timestamp: '2025-01-01T00:00:00Z' })
      dispatchAlert('signal_update', { severity: 'medium', message: 'Signal detected', timestamp: '2025-01-01T00:00:00Z' })
      dispatchAlert('heat_alert', { severity: 'low', message: 'Heat warning', timestamp: '2025-01-01T00:00:00Z' })
      dispatchAlert('quality_change', { message: 'Quality drop', timestamp: '2025-01-01T00:00:00Z' })
      dispatchAlert('consensus_alert', { message: 'Consensus alert', timestamp: '2025-01-01T00:00:00Z' })
    })

    // capped at maxAlerts = 2, so only the last two alerts remain
    expect(screen.getByText('Quality drop')).toBeInTheDocument()
    expect(screen.getByText('Consensus alert')).toBeInTheDocument()
    expect(screen.queryByText('Risk rising')).not.toBeInTheDocument()
    expect(screen.queryByText('Heat warning')).not.toBeInTheDocument()
  })

  it('uses fallback messages when absent', () => {
    render(<RealTimeAlerts />)
    act(() => {
      dispatchAlert('risk_update', { severity: 'high', timestamp: '2025-01-01T00:00:00Z' })
    })
    expect(screen.getByText('risk update detected')).toBeInTheDocument()
  })

  it('uses default icon and color for unknown types and severities', () => {
    render(<RealTimeAlerts />)
    act(() => {
      dispatchAlert('signal_update', { severity: 'medium', message: 'Signal alert', timestamp: 'x' })
      dispatchAlert('heat_alert', { severity: 'low', message: 'Heat alert', timestamp: 'x' })
      dispatchAlert('mystery_update', { severity: 'extreme', message: 'Mystery alert', timestamp: 'x' })
    })
    expect(screen.getByText('Signal alert')).toBeInTheDocument()
    expect(screen.getByText('Heat alert')).toBeInTheDocument()
    expect(screen.getByText('Mystery alert')).toBeInTheDocument()
  })

  it('dismisses alerts', () => {
    render(<RealTimeAlerts />)
    act(() => {
      dispatchAlert('risk_update', { severity: 'high', message: 'Dismiss me', timestamp: '2025-01-01T00:00:00Z' })
    })
    fireEvent.click(screen.getByRole('button'))
    expect(screen.queryByText('Dismiss me')).not.toBeInTheDocument()
  })

  it('removes the listener on unmount', () => {
    const { unmount } = render(<RealTimeAlerts />)
    unmount()
    act(() => {
      dispatchAlert('risk_update', { severity: 'high', message: 'Ghost', timestamp: 'x' })
    })
    expect(screen.queryByText('Ghost')).not.toBeInTheDocument()
  })
})

describe('RPCLatencyMini', () => {
  it('renders latency badge for each range', () => {
    const { container } = render(
      <RPCLatencyMini data={{ overall_avg: 20, overall_p95: 0, overall_p99: 0, error_rate: 0, endpoints: [] }} />
    )
    expect(container.textContent).toContain('20ms')
    render(<RPCLatencyMini data={{ overall_avg: 70, overall_p95: 0, overall_p99: 0, error_rate: 0, endpoints: [] }} />)
    render(<RPCLatencyMini data={{ overall_avg: 150, overall_p95: 0, overall_p99: 0, error_rate: 0, endpoints: [] }} />)
  })
})

describe('WalletAttribution', () => {
  const wallets = [
    {
      id: 1,
      address: 'wallet-aaaaaaaa',
      status: 'ACTIVE' as const,
      wqs_score: '70.5',
      roi_30d: '12.3',
      trade_count_30d: 10,
      win_rate: '0.6',
    },
    {
      id: 2,
      address: 'wallet-bbbbbbbb',
      status: 'CANDIDATE' as const,
      wqs_score: '45.0',
      roi_30d: '-5.0',
      trade_count_30d: null,
      win_rate: null,
    },
    {
      id: 3,
      address: 'wallet-cccccccc',
      status: 'REJECTED' as const,
      wqs_score: null,
      roi_30d: null,
      trade_count_30d: 0,
      win_rate: null,
    },
  ]

  it('sorts by ROI and renders wallet rows', () => {
    const { container } = render(<WalletAttribution wallets={wallets} />)
    expect(container.textContent).toContain('wallet-a...aaaaaaaa')
    expect(screen.getAllByText('N/A').length).toBeGreaterThan(0)
    expect(screen.getByText('CANDIDATE')).toBeInTheDocument()
  })

  it('renders an empty list', () => {
    const { container } = render(<WalletAttribution wallets={[]} />)
    expect(container.querySelector('tbody')?.children).toHaveLength(0)
  })
})
