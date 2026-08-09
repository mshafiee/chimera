import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen } from '@testing-library/react'
import { Dashboard } from '../Dashboard'

vi.mock('recharts', async () => await import('../../test-utils/rechartsMock'))

const apiMock = vi.hoisted(() => ({
  useHealth: vi.fn(),
  usePositions: vi.fn(),
  useWallets: vi.fn(),
  usePortfolioRisk: vi.fn(),
  useRPCLatency: vi.fn(),
  useCostAnalysis: vi.fn(),
  useBalanceAndNAV: vi.fn(),
  useNavHistory: vi.fn(),
}))

const metricsMock = vi.hoisted(() => ({
  useCostMetrics: vi.fn(),
  usePerformanceMetrics: vi.fn(),
  useStrategyPerformance: vi.fn(),
}))

const tradesMock = vi.hoisted(() => ({ useTrades: vi.fn() }))
const configMock = vi.hoisted(() => ({ useConfig: vi.fn() }))
const useWebSocketMock = vi.hoisted(() => vi.fn())
const useLayoutContextMock = vi.hoisted(() => vi.fn())

vi.mock('../../api', () => apiMock)
vi.mock('../../api/metrics', () => metricsMock)
vi.mock('../../api/trades', () => tradesMock)
vi.mock('../../api/config', () => configMock)
vi.mock('../../hooks/useWebSocket', () => ({ useWebSocket: useWebSocketMock }))
vi.mock('../../components/layout/Layout', () => ({
  useLayoutContext: useLayoutContextMock,
}))

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

const health = {
  status: 'healthy',
  uptime_seconds: 90000,
  queue_depth: 5,
  rpc_latency_ms: 10,
  last_trade_at: null,
  database: { status: 'healthy', message: null },
  rpc: { status: 'healthy', message: null },
  circuit_breaker: {
    state: 'TRIPPED',
    trading_allowed: false,
    trip_reason: 'loss limit',
    cooldown_remaining_secs: 125,
  },
  price_cache: { total_entries: 1, tracked_tokens: 1 },
}

function setupData(overrides: Record<string, unknown> = {}) {
  const position = {
    trade_uuid: 'pos-1',
    wallet_address: 'w1',
    token_address: 'tok-1',
    token_symbol: 'T1',
    strategy: 'SHIELD',
    entry_amount_sol: '2.5',
    entry_price: '1.5',
    entry_tx_signature: 'sig-entry',
    current_price: '1.6',
    unrealized_pnl_sol: '0.25',
    unrealized_pnl_percent: '6.6',
    state: 'ACTIVE',
    exit_price: null,
    exit_tx_signature: null,
    realized_pnl_sol: null,
    realized_pnl_usd: null,
    opened_at: '2025-01-01T00:00:00Z',
    last_updated: '2025-01-01T00:00:00Z',
    closed_at: null,
  }
  const closed = {
    ...position,
    trade_uuid: 'pos-2',
    state: 'CLOSED',
    closed_at: new Date(Date.now() - 3600000).toISOString(),
    realized_pnl_sol: '1.2',
    exit_tx_signature: 'sig-exit',
  }
  apiMock.useHealth.mockReturnValue({
    data: health,
    error: null,
    refetch: vi.fn(),
  })
  apiMock.usePositions.mockReturnValue({
    data: { positions: [position, closed], total: 2, total_unrealized_pnl_sol: '0.25' },
    error: null,
    isLoading: false,
    refetch: vi.fn(),
  })
  apiMock.useWallets.mockReturnValue({
    data: { wallets: [{ address: 'wallet-1', status: 'ACTIVE', wqs_score: '80', roi_30d: '10', trade_count_30d: 5, win_rate: '0.6' }], total: 1 },
    error: null,
  })
  apiMock.usePortfolioRisk.mockReturnValue({
    data: {
      portfolio_heat_percent: 60,
      heat_threshold: 80,
      heat_status: 'elevated',
      concentration: { by_token: [], by_sector: [], max_concentration_percent: 20, hhi: 1000 },
      exposure: { total_exposure_sol: 10, long_exposure_sol: 10, short_exposure_sol: 0, net_exposure_sol: 10, max_drawdown_percent: 10, current_drawdown_percent: 2 },
      drawdown: { current_drawdown_percent: 2, max_drawdown_percent: 10, drawdown_duration_days: 2, recovery_percent: 80 },
      total_capital_sol: 100,
      wallet_balance_sol: 90,
    },
    error: null,
  })
  apiMock.useRPCLatency.mockReturnValue({
    data: {
      overall_avg: 30,
      overall_p95: 50,
      overall_p99: 80,
      error_rate: 0.5,
      endpoints: [{ endpoint: 'https://rpc', avg_latency_ms: 30, p95_latency_ms: 50, p99_latency_ms: 80, error_rate: 0.5, request_count: 10, success_rate: 0.99 }],
    },
    error: null,
  })
  apiMock.useCostAnalysis.mockReturnValue({
    data: {
      per_trade_costs: [{ trade_uuid: 't1', timestamp: 'x', token_symbol: 'T1', jito_tip_sol: 0.001, dex_fee_sol: 0.002, slippage_cost_sol: 0.003, total_cost_sol: 0.006, execution_time_ms: 100 }],
      cost_by_type: [{ type: 'jito_tip', total_sol: 0.001, average_sol: 0.001, percentage: 50 }],
      optimization_opportunities: [],
      total_costs: 0.006,
      avg_cost_per_trade: 0.006,
    },
    error: null,
  })
  apiMock.useBalanceAndNAV.mockReturnValue({ balance: 90, nav: 92, isLoading: false })
  apiMock.useNavHistory.mockReturnValue({
    data: {
      points: [{ recorded_at: '2025-01-01T00:00:00Z', nav_sol: 100, capital_sol: 90, realized_pnl_sol: 5, unrealized_pnl_sol: 5, open_positions: 2 }],
      latest_nav_sol: 100,
      latest_unrealized_pnl_sol: 5,
    },
    isLoading: false,
  })
  metricsMock.usePerformanceMetrics.mockReturnValue({
    data: { pnl_24h: 5, pnl_7d: 10, pnl_30d: 20, pnl_24h_change_percent: 2.5, pnl_7d_change_percent: null, pnl_30d_change_percent: null },
    error: null,
    isLoading: false,
  })
  metricsMock.useCostMetrics.mockReturnValue({
    data: { avg_jito_tip_sol: '0.001', avg_dex_fee_sol: '0.002', avg_slippage_cost_sol: '0.003', total_costs_30d_sol: '0.006', net_profit_30d_sol: '5.5', roi_percent: '3.2' },
    error: null,
    isLoading: false,
  })
  metricsMock.useStrategyPerformance.mockReturnValue({
    data: { strategy: 'SHIELD', win_rate: 60, avg_return: 1.2, trade_count: 10, total_pnl: 12 },
    error: null,
  })
  tradesMock.useTrades.mockReturnValue({
    data: {
      trades: [
        { trade_uuid: 'tr-1', wallet_address: 'w1', token_address: 't1', token_symbol: 'T1', strategy: 'SHIELD', side: 'BUY', amount_sol: '1', price_at_signal: '2', tx_signature: null, status: 'CLOSED', retry_count: 0, error_message: null, pnl_sol: '0.5', pnl_usd: '1', created_at: '2025-01-01T00:00:00Z', updated_at: '2025-01-01T00:00:00Z' },
        { trade_uuid: 'tr-2', wallet_address: 'w1', token_address: 't2', token_symbol: 'T2', strategy: 'SPEAR', side: 'SELL', amount_sol: '2', price_at_signal: '3', tx_signature: null, status: 'CLOSED', retry_count: 0, error_message: null, pnl_sol: '-0.3', pnl_usd: '-0.6', created_at: '2025-01-02T00:00:00Z', updated_at: '2025-01-02T00:00:00Z' },
      ],
      total: 2,
      limit: 1000,
      offset: 0,
    },
    isLoading: false,
  })
  configMock.useConfig.mockReturnValue({
    data: {
      strategy_allocation: { shield_percent: 70, spear_percent: 30 },
      rpc_status: { primary: 'helius', active: 'jito', fallback_triggered: false },
      jito_enabled: true,
    },
    error: null,
  })
  useWebSocketMock.mockReturnValue({
    isConnected: true,
    lastMessage: overrides.lastMessage ?? null,
  })
  useLayoutContextMock.mockReturnValue({ setLastUpdate: vi.fn() })
}

beforeEach(() => {
  vi.clearAllMocks()
  setupData()
})

describe('Dashboard', () => {
  it('renders the full dashboard with data', () => {
    render(<Dashboard />)
    expect(screen.getByText(/Trading Halted/)).toBeInTheDocument()
    expect(screen.getByText('System Health')).toBeInTheDocument()
    expect(screen.getByText('Portfolio Risk')).toBeInTheDocument()
    expect(screen.getByText('RPC Latency')).toBeInTheDocument()
    expect(screen.getByText(/Cost Breakdown/)).toBeInTheDocument()
    expect(screen.getByText(/Top Performing Wallets/)).toBeInTheDocument()
    expect(screen.getByText('Live Positions')).toBeInTheDocument()
    expect(screen.getAllByText('$T1').length).toBeGreaterThan(0)
    expect(screen.getByText(/Shield Strategy/)).toBeInTheDocument()
    expect(screen.getByText(/Spear Strategy/)).toBeInTheDocument()
  })

  it('renders without a halted banner when trading is allowed', () => {
    apiMock.useHealth.mockReturnValue({
      data: { ...health, circuit_breaker: { ...health.circuit_breaker, trading_allowed: true, cooldown_remaining_secs: 0 } },
      error: null,
      refetch: vi.fn(),
    })
    render(<Dashboard />)
    expect(screen.queryByText(/Trading Halted/)).not.toBeInTheDocument()
  })

  it('shows loading states', () => {
    apiMock.usePositions.mockReturnValue({ data: undefined, error: null, isLoading: true, refetch: vi.fn() })
    metricsMock.usePerformanceMetrics.mockReturnValue({ data: undefined, error: null, isLoading: true })
    apiMock.useNavHistory.mockReturnValue({ data: undefined, isLoading: true })
    metricsMock.useCostMetrics.mockReturnValue({ data: undefined, error: null, isLoading: true })
    render(<Dashboard />)
    expect(screen.getByText('Loading positions...')).toBeInTheDocument()
    expect(screen.getByText('Loading NAV history…')).toBeInTheDocument()
    expect(screen.getByText('Loading cost metrics...')).toBeInTheDocument()
  })

  it('renders empty states', () => {
    apiMock.usePositions.mockReturnValue({ data: undefined, error: null, isLoading: false, refetch: vi.fn() })
    tradesMock.useTrades.mockReturnValue({ data: undefined })
    apiMock.useNavHistory.mockReturnValue({ data: undefined, isLoading: false })
    metricsMock.useCostMetrics.mockReturnValue({ data: undefined, error: null, isLoading: false })
    metricsMock.useStrategyPerformance.mockReturnValue({ data: undefined, error: null })
    apiMock.useWallets.mockReturnValue({ data: undefined, error: null })
    apiMock.usePortfolioRisk.mockReturnValue({ data: undefined, error: null })
    apiMock.useRPCLatency.mockReturnValue({ data: undefined, error: null })
    apiMock.useCostAnalysis.mockReturnValue({ data: undefined, error: null })
    render(<Dashboard />)
    expect(screen.getByText('No positions yet')).toBeInTheDocument()
    expect(screen.getByText(/Collecting NAV data/)).toBeInTheDocument()
    expect(screen.getByText('No cost data available')).toBeInTheDocument()
    expect(screen.getByText('No trade history available')).toBeInTheDocument()
  })

  it('renders degraded health and negative metric changes', () => {
    apiMock.useHealth.mockReturnValue({
      data: { ...health, status: 'degraded' },
      error: null,
      refetch: vi.fn(),
    })
    metricsMock.usePerformanceMetrics.mockReturnValue({
      data: { pnl_24h: -5, pnl_7d: -10, pnl_30d: -20, pnl_24h_change_percent: -1.5, pnl_7d_change_percent: 3.5, pnl_30d_change_percent: 4.5 },
      error: null,
      isLoading: false,
    })
    const { container } = render(<Dashboard />)
    expect(container.textContent).toContain('degraded')
    expect(container.textContent).toContain('+3.5%')
    expect(container.textContent).toContain('+4.5%')
  })

  it('renders undefined metric changes and large uptimes', () => {
    apiMock.useHealth.mockReturnValue({
      data: { ...health, uptime_seconds: 3 * 86400 + 7200 + 60 },
      error: null,
      refetch: vi.fn(),
    })
    metricsMock.usePerformanceMetrics.mockReturnValue({
      data: { pnl_24h: 5, pnl_7d: 10, pnl_30d: 20 },
      error: null,
      isLoading: false,
    })
    const { container, rerender } = render(<Dashboard />)
    expect(container.textContent).toContain('3d 2h 1m')

    apiMock.useHealth.mockReturnValue({
      data: { ...health, uptime_seconds: 7200 + 60 },
      error: null,
      refetch: vi.fn(),
    })
    metricsMock.usePerformanceMetrics.mockReturnValue({
      data: { pnl_24h: 5, pnl_7d: 10, pnl_30d: 20, pnl_24h_change_percent: 2.5, pnl_7d_change_percent: 3.5, pnl_30d_change_percent: 4.5 },
      error: null,
      isLoading: false,
    })
    rerender(<Dashboard />)
    expect(container.textContent).toContain('2h 1m')
    expect(container.textContent).toContain('+2.5%')
    expect(container.textContent).toContain('+3.5%')
    expect(container.textContent).toContain('+4.5%')
  })

  it('renders helius and jito health variants', () => {
    configMock.useConfig.mockReturnValue({
      data: {
        strategy_allocation: { shield_percent: 70, spear_percent: 30 },
        rpc_status: { primary: 'other', active: 'other', fallback_triggered: true },
        jito_enabled: true,
      },
      error: null,
    })
    const { container } = render(<Dashboard />)
    expect(container.textContent).toContain('Helius')
    expect(container.textContent).toContain('Jito')

    configMock.useConfig.mockReturnValue({
      data: {
        strategy_allocation: { shield_percent: 70, spear_percent: 30 },
        rpc_status: { primary: 'helius', active: 'jito', fallback_triggered: true },
        jito_enabled: false,
      },
      error: null,
    })
    const { container: c2 } = render(<Dashboard />)
    expect(c2.textContent).toContain('Helius')
  })

  it('renders positions with null values and long-closed positions', () => {
    const closedLongAgo = {
      trade_uuid: 'pos-3',
      wallet_address: 'w1',
      token_address: 'tok-3',
      token_symbol: 'T3',
      strategy: 'SPEAR',
      entry_amount_sol: '1',
      entry_price: '1',
      entry_tx_signature: 's3',
      current_price: null,
      unrealized_pnl_percent: null,
      state: 'ACTIVE',
      exit_price: null,
      exit_tx_signature: null,
      realized_pnl_sol: null,
      realized_pnl_usd: null,
      opened_at: '2025-01-01T00:00:00Z',
      last_updated: '2025-01-01T00:00:00Z',
      closed_at: null,
    }
    const oldClosed = {
      ...closedLongAgo,
      trade_uuid: 'pos-4',
      state: 'CLOSED',
      closed_at: new Date(Date.now() - 3 * 86400 * 1000).toISOString(),
      realized_pnl_sol: null,
    }
    apiMock.usePositions.mockReturnValue({
      data: { positions: [closedLongAgo, oldClosed], total: 2, total_unrealized_pnl_sol: null },
      error: null,
      isLoading: false,
      refetch: vi.fn(),
    })
    const { container } = render(<Dashboard />)
    expect(container.textContent).toContain('$T3')
    expect(container.textContent).toContain('-')
  })

  it('shows the API error banner when requests fail', () => {
    apiMock.useHealth.mockReturnValue({ data: undefined, error: new Error('health down'), refetch: vi.fn() })
    render(<Dashboard />)
    expect(screen.getByText(/Some data may be stale/)).toBeInTheDocument()
  })

  it('handles critical, warning and info alert messages', () => {
    setupData({ lastMessage: { type: 'alert', data: { severity: 'critical', message: 'Critical alert!' } } })
    const { rerender } = render(<Dashboard />)
    expect(toastMock.error).toHaveBeenCalledWith('Critical alert!', 10000)

    setupData({ lastMessage: { type: 'alert', data: { severity: 'warning', message: 'Warning alert' } } })
    rerender(<Dashboard />)
    expect(toastMock.warning).toHaveBeenCalledWith('Warning alert')

    setupData({ lastMessage: { type: 'alert', data: { severity: 'info', message: 'Info alert' } } })
    rerender(<Dashboard />)
    expect(toastMock.info).toHaveBeenCalledWith('Info alert')

    setupData({ lastMessage: { type: 'alert', data: {} } })
    rerender(<Dashboard />)
    expect(toastMock.info).toHaveBeenCalledWith('Alert received')
  })

  it('refetches positions and health on websocket messages', () => {
    const refetchPositions = vi.fn()
    const refetchHealth = vi.fn()
    apiMock.usePositions.mockReturnValue({
      data: { positions: [], total: 0, total_unrealized_pnl_sol: null },
      error: null,
      isLoading: false,
      refetch: refetchPositions,
    })
    apiMock.useHealth.mockReturnValue({ data: health, error: null, refetch: refetchHealth })

    const { rerender } = render(<Dashboard />)
    // first render: no message yet
    expect(refetchPositions).not.toHaveBeenCalled()
    expect(refetchHealth).not.toHaveBeenCalled()

    apiMock.usePositions.mockReturnValue({
      data: { positions: [], total: 0, total_unrealized_pnl_sol: null },
      error: null,
      isLoading: false,
      refetch: refetchPositions,
    })
    apiMock.useHealth.mockReturnValue({ data: health, error: null, refetch: refetchHealth })
    useWebSocketMock.mockReturnValue({ isConnected: true, lastMessage: { type: 'position_update', data: {} } })
    rerender(<Dashboard />)
    expect(refetchPositions).toHaveBeenCalled()

    useWebSocketMock.mockReturnValue({ isConnected: true, lastMessage: { type: 'health_update', data: {} } })
    rerender(<Dashboard />)
    expect(refetchHealth).toHaveBeenCalled()
  })
})
