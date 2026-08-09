import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { Performance } from '../Performance'

vi.mock('recharts', async () => await import('../../test-utils/rechartsMock'))

const apiMock = vi.hoisted(() => ({
  useTradeLatency: vi.fn(),
  useRPCLatency: vi.fn(),
  useDatabasePerformance: vi.fn(),
  useRequestRate: vi.fn(),
  useCostAnalysis: vi.fn(),
}))

vi.mock('../../api', () => apiMock)

const latency = {
  p50: 10,
  p95: 50,
  p99: 100,
  max: 200,
  avg: 30,
  histogram: [{ range: '0-10ms', count: 5, percentage: 50 }],
}

const rpc = {
  overall_avg: 30,
  overall_p95: 50,
  overall_p99: 80,
  error_rate: 2,
  endpoints: [{ endpoint: 'https://rpc', avg_latency_ms: 30, p95_latency_ms: 50, p99_latency_ms: 80, error_rate: 2, request_count: 10, success_rate: 0.98 }],
}

const db = {
  query_latency: { avg_ms: 10, p95_ms: 20, p99_ms: 30, slow_queries: 0, total_queries: 100 },
  connection_pool: { active_connections: 5, idle_connections: 3, max_connections: 10, utilization_percent: 50 },
  cache_performance: { hit_rate: 0.95, miss_rate: 0.05, total_hits: 1000, total_misses: 50, size: 100, max_size: 200 },
}

const rate = {
  current_rps: 10,
  peak_rps: 20,
  avg_rps: 5,
  overall_status: 'healthy',
  rate_limits: [{ endpoint: '/api/v1/trades', current_rate: 8, limit: 10, utilization_percent: 80, window_seconds: 60, remaining: 2, reset_at: 'x', status: 'ok' }],
}

const costs = {
  per_trade_costs: [{ trade_uuid: 't1', timestamp: 'x', token_symbol: 'SOL', jito_tip_sol: 0.001, dex_fee_sol: 0.002, slippage_cost_sol: 0.003, total_cost_sol: 0.006, execution_time_ms: 100 }],
  cost_by_type: [{ type: 'jito_tip', total_sol: 0.001, average_sol: 0.001, percentage: 50 }],
  optimization_opportunities: [],
  total_costs: 0.006,
  avg_cost_per_trade: 0.006,
}

function setup() {
  apiMock.useTradeLatency.mockReturnValue({ data: latency, isLoading: false, error: null })
  apiMock.useRPCLatency.mockReturnValue({ data: rpc, isLoading: false })
  apiMock.useDatabasePerformance.mockReturnValue({ data: db, isLoading: false })
  apiMock.useRequestRate.mockReturnValue({ data: rate, isLoading: false })
  apiMock.useCostAnalysis.mockReturnValue({ data: costs, isLoading: false })
}

beforeEach(() => {
  vi.clearAllMocks()
  setup()
})

describe('Performance', () => {
  it('renders the full performance page', () => {
    render(<Performance />)
    expect(screen.getByText('Performance Analytics')).toBeInTheDocument()
    expect(screen.getByText('Trade Execution Latency')).toBeInTheDocument()
    expect(screen.getAllByText('10ms').length).toBeGreaterThan(0)
    expect(screen.getByText('RPC Endpoint Latency')).toBeInTheDocument()
    expect(screen.getByText('Database Performance')).toBeInTheDocument()
    expect(screen.getByText('Request Rate')).toBeInTheDocument()
    expect(screen.getByText('Cost Analysis (Per-Trade Breakdown)')).toBeInTheDocument()
    expect(screen.getByText('$SOL')).toBeInTheDocument()
  })

  it('renders loading states', () => {
    apiMock.useTradeLatency.mockReturnValue({ data: undefined, isLoading: true, error: null })
    apiMock.useRPCLatency.mockReturnValue({ data: undefined, isLoading: true })
    apiMock.useDatabasePerformance.mockReturnValue({ data: undefined, isLoading: true })
    apiMock.useRequestRate.mockReturnValue({ data: undefined, isLoading: true })
    apiMock.useCostAnalysis.mockReturnValue({ data: undefined, isLoading: true })
    render(<Performance />)
    expect(screen.getByText('Loading latency data...')).toBeInTheDocument()
    expect(screen.getByText('Loading RPC data...')).toBeInTheDocument()
    expect(screen.getByText('Loading database data...')).toBeInTheDocument()
    expect(screen.getByText('Loading request rate...')).toBeInTheDocument()
    expect(screen.getByText('Loading cost data...')).toBeInTheDocument()
  })

  it('renders empty states', () => {
    apiMock.useTradeLatency.mockReturnValue({ data: undefined, isLoading: false, error: null })
    apiMock.useRPCLatency.mockReturnValue({ data: undefined, isLoading: false })
    apiMock.useDatabasePerformance.mockReturnValue({ data: undefined, isLoading: false })
    apiMock.useRequestRate.mockReturnValue({ data: undefined, isLoading: false })
    apiMock.useCostAnalysis.mockReturnValue({ data: undefined, isLoading: false })
    render(<Performance />)
    expect(screen.getByText('No Performance Metrics Available Yet')).toBeInTheDocument()
    expect(screen.getByText('No latency metrics available yet')).toBeInTheDocument()
    expect(screen.getByText('No RPC data available')).toBeInTheDocument()
    expect(screen.getByText('No database data available')).toBeInTheDocument()
    expect(screen.getByText('No request rate data available')).toBeInTheDocument()
    expect(screen.getByText('No cost data available')).toBeInTheDocument()
  })

  it('renders the latency error state', () => {
    apiMock.useTradeLatency.mockReturnValue({ data: undefined, isLoading: false, error: new Error('down') })
    render(<Performance />)
    expect(screen.getByText('Error loading latency data')).toBeInTheDocument()
  })

  it('changes the time range', () => {
    render(<Performance />)
    fireEvent.click(screen.getByText('7D'))
    expect(apiMock.useTradeLatency).toHaveBeenCalled()
  })

  it('tolerates proxy-wrapped latency data with missing keys', () => {
    const proxy = new Proxy({ p50: 10, p95: 20, p99: 30, avg: 15, histogram: [{ range: 'a', count: 1, percentage: 100 }] }, {
      has(target, key) {
        if (key === 'p50') return Reflect.has(target, key)
        if (key === 'avg') return false
        throw new Error('boom')
      },
    }) as object
    apiMock.useTradeLatency.mockReturnValue({ data: proxy, isLoading: false, error: null })
    const { container } = render(<Performance />)
    expect(container.textContent).toContain('ms')
  })
})
