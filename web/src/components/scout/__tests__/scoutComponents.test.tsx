import { describe, it, expect } from 'vitest'
import { render, screen } from '@testing-library/react'
import { ScoutStatusCard } from '../ScoutStatusCard'
import { WQSDistributionChart } from '../WQSDistributionChart'
import * as scoutBarrel from '../index'

vi.mock('recharts', async () => await import('../../../test-utils/rechartsMock'))

const scoutStatus = {
  last_run_at: '2025-01-01T00:00:00Z',
  next_run_at: '2025-01-02T00:00:00Z',
  wallets_analyzed: 42,
  analysis_duration_seconds: 12.5,
  status: 'running' as const,
  wqs_distribution: [],
  promotion_queue: [],
  rejection_queue: [],
}

describe('scout barrel', () => {
  it('re-exports all components', () => {
    expect(scoutBarrel.ScoutStatusCard).toBeTruthy()
    expect(scoutBarrel.WQSDistributionChart).toBeTruthy()
  })
})

describe('ScoutStatusCard', () => {
  it('renders the loading state', () => {
    render(<ScoutStatusCard status={null} isLoading />)
    expect(screen.getByText('Loading Scout status...')).toBeInTheDocument()
  })

  it('renders the no-status state', () => {
    render(<ScoutStatusCard status={null} />)
    expect(screen.getByText('No Scout status available')).toBeInTheDocument()
  })

  it('renders a running status with next run', () => {
    render(<ScoutStatusCard status={scoutStatus} />)
    expect(screen.getByText('running')).toBeInTheDocument()
    expect(screen.getByText('42 wallets')).toBeInTheDocument()
    expect(screen.getByText('12.5s')).toBeInTheDocument()
    expect(screen.getByText(/Next scheduled run/)).toBeInTheDocument()
  })

  it('renders completed/failed/idle statuses without dates', () => {
    for (const status of ['completed', 'failed', 'idle'] as const) {
      render(
        <ScoutStatusCard
          status={{ ...scoutStatus, status, last_run_at: null, next_run_at: null }}
        />
      )
    }
    expect(screen.getByText('completed')).toBeInTheDocument()
    expect(screen.getByText('failed')).toBeInTheDocument()
    expect(screen.getByText('idle')).toBeInTheDocument()
    expect(screen.getAllByText('Never').length).toBeGreaterThan(0)
  })
})

describe('WQSDistributionChart', () => {
  it('renders stats and distribution chart', () => {
    const { _container } = render(
      <WQSDistributionChart
        data={{
          distribution: [
            { range: '0-20', count: 5, percentage: 5 },
            { range: '20-40', count: 10, percentage: 10 },
            { range: '40-60', count: 20, percentage: 20 },
            { range: '60-80', count: 30, percentage: 30 },
            { range: '80-100', count: 35, percentage: 35 },
          ],
          average_score: 62.5,
          median_score: 65,
          total_wallets: 100,
        }}
      />
    )
    expect(screen.getByText('62.5')).toBeInTheDocument()
    expect(screen.getByText('65.0')).toBeInTheDocument()
    expect(screen.getByText('100')).toBeInTheDocument()
  })

  it('falls back to gray for unknown ranges', () => {
    const { container } = render(
      <WQSDistributionChart
        data={{ distribution: [{ range: '100-120', count: 1, percentage: 1 }], average_score: 0, median_score: 0, total_wallets: 1 }}
      />
    )
    expect(container).toBeDefined()
  })
})
