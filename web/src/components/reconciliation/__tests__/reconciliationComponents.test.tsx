import { describe, it, expect } from 'vitest'
import { render, screen } from '@testing-library/react'
import { DiscrepanciesList } from '../DiscrepanciesList'
import { ReconciliationHistory } from '../ReconciliationHistory'
import { ReconciliationStatusCard } from '../ReconciliationStatusCard'
import * as reconciliationBarrel from '../index'
import type { Discrepancy, ReconciliationRun } from '../../api'

const discrepancies: Discrepancy[] = [
  {
    id: 1,
    trade_uuid: 'trade-uuid-1',
    type: 'missing_position',
    severity: 'critical',
    description: 'Position missing on chain',
    db_value: 'x',
    on_chain_value: 'y',
    detected_at: '2025-01-01T00:00:00Z',
    resolved: false,
    resolved_at: null,
  },
  {
    id: 2,
    trade_uuid: 'trade-uuid-2',
    type: 'pnl_mismatch',
    severity: 'low',
    description: 'PnL differs',
    db_value: null,
    on_chain_value: null,
    detected_at: '2025-01-02T00:00:00Z',
    resolved: true,
    resolved_at: '2025-01-03T00:00:00Z',
  },
]

const runs: ReconciliationRun[] = [
  { id: 1, started_at: '2025-01-01T00:00:00Z', completed_at: '2025-01-01T00:00:01Z', status: 'completed', checked_count: 10, discrepancy_count: 0, unresolved_count: 0, duration_seconds: 12.3 },
  { id: 2, started_at: '2025-01-02T00:00:00Z', completed_at: null, status: 'running', checked_count: 5, discrepancy_count: 1, unresolved_count: 1, duration_seconds: null },
]

describe('reconciliation barrel', () => {
  it('re-exports all components', () => {
    expect(reconciliationBarrel.ReconciliationStatusCard).toBeTruthy()
    expect(reconciliationBarrel.DiscrepanciesList).toBeTruthy()
    expect(reconciliationBarrel.ReconciliationHistory).toBeTruthy()
  })
})

describe('DiscrepanciesList', () => {
  it('renders discrepancy rows', () => {
    render(<DiscrepanciesList discrepancies={discrepancies} />)
    expect(screen.getByText('Open')).toBeInTheDocument()
    expect(screen.getByText('Resolved')).toBeInTheDocument()
    expect(screen.getByText('Missing Position')).toBeInTheDocument()
    expect(screen.getByText('PnL Mismatch')).toBeInTheDocument()
    expect(screen.getAllByText('trade-uu...').length).toBe(2)
  })

  it('renders unknown types with fallback labels and missing values', () => {
    render(
      <DiscrepanciesList
        discrepancies={[
          { ...discrepancies[0], id: 9, type: 'unknown_type' as never, trade_uuid: 'short', db_value: null, on_chain_value: null },
        ]}
      />
    )
    expect(screen.getByText('unknown_type')).toBeInTheDocument()
    expect(screen.getAllByText('—').length).toBe(2)
  })

  it('renders an empty list', () => {
    const { container } = render(<DiscrepanciesList discrepancies={[]} />)
    expect(container.querySelector('tbody')?.children).toHaveLength(0)
  })
})

describe('ReconciliationHistory', () => {
  it('renders run rows with statuses and durations', () => {
    render(<ReconciliationHistory runs={runs} />)
    expect(screen.getByText('completed')).toBeInTheDocument()
    expect(screen.getByText('running')).toBeInTheDocument()
    expect(screen.getByText('12.3s')).toBeInTheDocument()
    expect(screen.getByText('—')).toBeInTheDocument()
  })

  it('renders failed and pending runs', () => {
    render(
      <ReconciliationHistory
        runs={[
          { ...runs[0], id: 3, status: 'failed', unresolved_count: 3 },
          { ...runs[0], id: 4, status: 'pending', unresolved_count: 0 },
        ]}
      />
    )
    expect(screen.getByText('failed')).toBeInTheDocument()
    expect(screen.getByText('pending')).toBeInTheDocument()
  })
})

describe('ReconciliationStatusCard', () => {
  const status = {
    last_reconciliation_at: '2025-01-01T00:00:00Z',
    next_reconciliation_at: '2025-01-02T00:00:00Z',
    status: 'completed' as const,
    checked_count: 10,
    discrepancy_count: 1,
    unresolved_count: 1,
    duration_seconds: 5.5,
    recent_discrepancies: [],
  }

  it('renders the loading state', () => {
    render(<ReconciliationStatusCard status={null} isLoading />)
    expect(screen.getByText('Loading reconciliation status...')).toBeInTheDocument()
  })

  it('renders the no-status state', () => {
    render(<ReconciliationStatusCard status={null} />)
    expect(screen.getByText('No reconciliation status available')).toBeInTheDocument()
  })

  it('renders completed status with dates', () => {
    render(<ReconciliationStatusCard status={status} />)
    expect(screen.getByText('Completed')).toBeInTheDocument()
    expect(screen.getByText('5.5s')).toBeInTheDocument()
  })

  it('renders running/failed/pending statuses without dates', () => {
    for (const s of ['running', 'failed', 'pending'] as const) {
      render(
        <ReconciliationStatusCard
          status={{ ...status, status: s, last_reconciliation_at: null, next_reconciliation_at: null, duration_seconds: null }}
        />
      )
    }
    expect(screen.getByText('Running')).toBeInTheDocument()
    expect(screen.getByText('Failed')).toBeInTheDocument()
    expect(screen.getByText('Pending')).toBeInTheDocument()
  })
})
