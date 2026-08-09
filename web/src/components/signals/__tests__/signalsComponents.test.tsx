import { describe, it, expect } from 'vitest'
import { render, screen } from '@testing-library/react'
import { ConsensusMatrix } from '../ConsensusMatrix'
import { SignalQualityChart } from '../SignalQualityChart'
import { SignalSourcesTable } from '../SignalSourcesTable'
import * as signalsBarrel from '../index'

vi.mock('recharts', async () => await import('../../../test-utils/rechartsMock'))

describe('signals barrel', () => {
  it('re-exports all components', () => {
    expect(signalsBarrel.SignalQualityChart).toBeTruthy()
    expect(signalsBarrel.SignalSourcesTable).toBeTruthy()
    expect(signalsBarrel.ConsensusMatrix).toBeTruthy()
  })
})

describe('ConsensusMatrix', () => {
  const data = {
    consensus_rate: 0.5,
    avg_clustering_coefficient: 0.5,
    active_clusters: [
      {
        id: 'c1',
        wallets: ['wallet-address-1', 'wallet-address-2'],
        signal_count: 5,
        avg_wqs: 75.5,
        last_activity: '2025-01-01T00:00:00Z',
        coherence: 0.9,
      },
      {
        id: 'c2',
        wallets: ['wallet-address-3'],
        signal_count: 2,
        avg_wqs: 55.5,
        last_activity: '2025-01-01T00:00:00Z',
        coherence: 0.5,
      },
      {
        id: 'c3',
        wallets: ['wallet-address-4'],
        signal_count: 1,
        avg_wqs: 40,
        last_activity: '2025-01-01T00:00:00Z',
        coherence: 0.3,
      },
    ],
    recent_signals: [],
    divergence_alerts: [],
  }

  it('renders the empty state', () => {
    render(<ConsensusMatrix data={{ ...data, active_clusters: [] }} />)
    expect(screen.getByText('No consensus data available')).toBeInTheDocument()
  })

  it('renders matrix cells with consensus colors', () => {
    render(<ConsensusMatrix data={data} />)
    expect(screen.getAllByText('90%').length).toBeGreaterThan(0)
    expect(screen.getAllByText('50%').length).toBeGreaterThan(0)
    expect(screen.getAllByText('5 signals').length).toBe(2)
    expect(screen.getAllByText('WQS: 75.5').length).toBe(2)
  })
})

describe('SignalQualityChart', () => {
  it('renders distribution, trend and summary stats', () => {
    const { _container } = render(
      <SignalQualityChart
        data={{
          current_quality_score: 0.7,
          quality_distribution: [
            { range: '0-0.2', count: 5, percentage: 5 },
            { range: '0.2-0.4', count: 15, percentage: 15 },
          ],
          rejection_rate: 0.2,
          total_signals: 100,
          accepted_signals: 80,
          rejected_signals: 20,
          average_quality_trend: [
            { timestamp: '2025-01-01T00:00:00Z', average_score: 0.6 },
            { timestamp: '2025-01-02T00:00:00Z', average_score: 0.65 },
          ],
        }}
      />
    )
    expect(screen.getByText('20.0%')).toBeInTheDocument()
    expect(screen.getByText('80.0%')).toBeInTheDocument()
  })

  it('renders without a trend', () => {
    const { container } = render(
      <SignalQualityChart
        data={{
          current_quality_score: 0.5,
          quality_distribution: [],
          rejection_rate: 0,
          total_signals: 0,
          accepted_signals: 0,
          rejected_signals: 0,
          average_quality_trend: [],
        }}
      />
    )
    expect(container).toBeDefined()
  })
})

describe('SignalSourcesTable', () => {
  it('sorts sources by signal count and renders rows', () => {
    render(
      <SignalSourcesTable
        sources={[
          { source: 'source-address-1', signal_count: 5, average_quality: 0.8, acceptance_rate: 0.9, last_signal_at: '2025-01-01T00:00:00Z' },
          { source: 'source-address-2', signal_count: 50, average_quality: 0.4, acceptance_rate: 0.3, last_signal_at: '2025-01-02T00:00:00Z' },
          { source: 'source-address-3', signal_count: 20, average_quality: 0.6, acceptance_rate: 0.6, last_signal_at: '' },
        ]}
      />
    )
    expect(screen.getByText('0.80')).toBeInTheDocument()
    expect(screen.getByText('0.40')).toBeInTheDocument()
    expect(screen.getByText('Never')).toBeInTheDocument()
    const rows = document.querySelectorAll('tbody tr')
    // highest signal_count first
    expect(rows[0].textContent).toContain('50')
  })
})
