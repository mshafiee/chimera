import { describe, it, expect } from 'vitest'
import { render, screen } from '@testing-library/react'
import { CostAnalysisChart } from '../CostAnalysisChart'
import { DatabasePerformanceCard } from '../DatabasePerformanceCard'
import { LatencyChart } from '../LatencyChart'
import { RequestRateCard } from '../RequestRateCard'
import { RPCLatencyTable } from '../RPCLatencyTable'
import * as performanceBarrel from '../index'

vi.mock('recharts', async () => await import('../../../test-utils/rechartsMock'))

const costData = {
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
  optimization_opportunities: [
    { type: 'tip', description: 'lower tips', potential_savings_sol: 0.5, current_value: 1, recommended_value: 0.5 },
  ],
  total_costs: 0.066,
  avg_cost_per_trade: 0.033,
}

describe('performance barrel', () => {
  it('re-exports all components', () => {
    expect(performanceBarrel.LatencyChart).toBeTruthy()
    expect(performanceBarrel.RPCLatencyTable).toBeTruthy()
    expect(performanceBarrel.DatabasePerformanceCard).toBeTruthy()
    expect(performanceBarrel.RequestRateCard).toBeTruthy()
    expect(performanceBarrel.CostAnalysisChart).toBeTruthy()
  })
})

describe('CostAnalysisChart', () => {
  it('renders the empty state', () => {
    const { _container } = render(
      <CostAnalysisChart
        data={{ per_trade_costs: [], cost_by_type: [], optimization_opportunities: [], total_costs: 0, avg_cost_per_trade: 0 }}
      />
    )
    expect(screen.getByText('No cost data available')).toBeInTheDocument()
  })

  it('renders full data with tables and opportunities', () => {
    render(<CostAnalysisChart data={costData} />)
    expect(screen.getByText('Total Costs')).toBeInTheDocument()
    expect(screen.getAllByText('Jito Tip').length).toBeGreaterThan(0)
    expect(screen.getByText('lower tips')).toBeInTheDocument()
    expect(screen.getByText('$SOL')).toBeInTheDocument()
    expect(screen.getByText('$Unknown')).toBeInTheDocument()
  })

  it('renders the inner empty chart state', () => {
    render(
      <CostAnalysisChart
        data={{ ...costData, per_trade_costs: [], cost_by_type: [], total_costs: 5 }}
      />
    )
    expect(screen.getAllByText('No cost data available')).toHaveLength(1)
  })
})

describe('DatabasePerformanceCard', () => {
  const data = {
    query_latency: { avg_ms: 10, p95_ms: 20, p99_ms: 30, slow_queries: 0, total_queries: 100 },
    connection_pool: { active_connections: 5, idle_connections: 3, max_connections: 10, utilization_percent: 50 },
    cache_performance: { hit_rate: 0.95, miss_rate: 0.05, total_hits: 1000, total_misses: 50, size: 100, max_size: 200 },
  }

  it('renders all metric groups', () => {
    render(<DatabasePerformanceCard data={data} />)
    expect(screen.getByText('Query Latency')).toBeInTheDocument()
    expect(screen.getByText('Connection Pool')).toBeInTheDocument()
    expect(screen.getByText('Cache Performance')).toBeInTheDocument()
    expect(screen.getByText('10ms')).toBeInTheDocument()
    expect(screen.getByText('1,000')).toBeInTheDocument()
  })

  it('renders with poor cache rates', () => {
    render(
      <DatabasePerformanceCard
        data={{
          ...data,
          cache_performance: { hit_rate: 0.4, miss_rate: 0.6, total_hits: 1, total_misses: 2, size: 1, max_size: 2 },
          connection_pool: { ...data.connection_pool, utilization_percent: 95 },
        }}
      />
    )
  })
})

describe('LatencyChart', () => {
  it('renders the empty state', () => {
    const { _container } = render(<LatencyChart data={{ p50: 0, p95: 0, p99: 0, max: 0, avg: 0, histogram: [] }} />)
    expect(screen.getByText('No latency data available')).toBeInTheDocument()
  })

  it('renders histogram data', () => {
    const { container } = render(
      <LatencyChart
        data={{
          p50: 10,
          p95: 50,
          p99: 100,
          max: 200,
          avg: 30,
          histogram: [
            { range: '0-10ms', count: 5, percentage: 50 },
            { range: '10-50ms', count: 5, percentage: 50 },
          ],
        }}
      />
    )
    expect(container).toBeDefined()
  })
})

describe('RequestRateCard', () => {
  const data = {
    current_rps: 10.5,
    peak_rps: 20,
    avg_rps: 5,
    overall_status: 'healthy' as const,
    rate_limits: [
      { endpoint: '/api/v1/trades', current_rate: 8, limit: 10, utilization_percent: 80, window_seconds: 60, remaining: 2, reset_at: 'x', status: 'ok' as const },
      { endpoint: '/api/v1/health', current_rate: 8, limit: 10, utilization_percent: 95, window_seconds: 60, remaining: 2, reset_at: 'x', status: 'warning' as const },
      { endpoint: '/api/v1/config', current_rate: 8, limit: 10, utilization_percent: 99, window_seconds: 60, remaining: 2, reset_at: 'x', status: 'throttled' as const },
    ],
  }

  it('renders stats and limit rows', () => {
    render(<RequestRateCard data={data} />)
    expect(screen.getByText('10.5')).toBeInTheDocument()
    expect(screen.getByText('/api/v1/trades')).toBeInTheDocument()
    expect(screen.getByText('80%')).toBeInTheDocument()
  })

  it('renders degraded/throttled statuses and empty limits', () => {
    render(
      <RequestRateCard data={{ ...data, overall_status: 'degraded', rate_limits: [] }} />
    )
    render(
      <RequestRateCard data={{ ...data, overall_status: 'throttled', rate_limits: [] }} />
    )
  })
})

describe('RPCLatencyTable', () => {
  const data = {
    overall_avg: 30,
    overall_p95: 60,
    overall_p99: 90,
    error_rate: 2,
    endpoints: [
      { endpoint: 'https://a', avg_latency_ms: 10, p95_latency_ms: 20, p99_latency_ms: 30, error_rate: 0.5, request_count: 100, success_rate: 0.995 },
      { endpoint: 'https://b', avg_latency_ms: 20, p95_latency_ms: 40, p99_latency_ms: 60, error_rate: 3, request_count: 50, success_rate: 0.97 },
      { endpoint: 'https://c', avg_latency_ms: 40, p95_latency_ms: 80, p99_latency_ms: 120, error_rate: 8, request_count: 10, success_rate: 0.9 },
    ],
  }

  it('renders overall stats and endpoint rows', () => {
    const { container } = render(<RPCLatencyTable data={data} />)
    expect(container.textContent).toContain('30ms')
    expect(screen.getByText('https://a')).toBeInTheDocument()
    expect(screen.getByText('99.50%')).toBeInTheDocument()
    expect(screen.getByText('97.00%')).toBeInTheDocument()
    expect(screen.getByText('90.00%')).toBeInTheDocument()
  })
})
