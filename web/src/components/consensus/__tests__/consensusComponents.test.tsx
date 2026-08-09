import { describe, it, expect } from 'vitest'
import { render, screen } from '@testing-library/react'
import { ConsensusOverview } from '../ConsensusOverview'
import { SignalAggregationView } from '../SignalAggregationView'
import { WalletClustersVisualization } from '../WalletClustersVisualization'
import * as consensusBarrel from '../index'
import type { ConsensusResponse, WalletClusteringResponse } from '../../api'
import type { SignalAggregationResponse } from '../../api/consensus'

const consensusData: ConsensusResponse = {
  consensus_rate: 0.8,
  avg_clustering_coefficient: 0.6,
  active_clusters: [
    {
      id: 'cluster-1',
      wallets: ['w1', 'w2'],
      signal_count: 5,
      avg_wqs: 70.5,
      last_activity: '2025-01-01T00:00:00Z',
      coherence: 0.85,
    },
    {
      id: 'cluster-2',
      wallets: ['w3'],
      signal_count: 2,
      avg_wqs: 55.5,
      last_activity: '2025-01-02T00:00:00Z',
      coherence: 0.6,
    },
  ],
  recent_signals: [],
  divergence_alerts: [],
}

const aggregationData: SignalAggregationResponse = {
  window_start: '2025-01-01T00:00:00Z',
  window_end: '2025-01-01T05:00:00Z',
  total_signals: 10,
  unique_tokens: 3,
  aggregated_signals: [
    {
      token_address: 'tok-abc',
      token_symbol: 'ABC',
      signal_count: 4,
      unique_wallets: 2,
      consensus_score: 0.8,
      recommended_action: 'BUY',
      confidence: 0.9,
    },
    {
      token_address: 'tok-def',
      token_symbol: null,
      signal_count: 2,
      unique_wallets: 1,
      consensus_score: 0.4,
      recommended_action: 'HOLD',
      confidence: 0.5,
    },
    {
      token_address: 'tok-ghi',
      token_symbol: 'GHI',
      signal_count: 1,
      unique_wallets: 1,
      consensus_score: 0.1,
      recommended_action: 'SELL',
      confidence: 0.2,
    },
    {
      token_address: 'tok-jkl',
      token_symbol: 'JKL',
      signal_count: 1,
      unique_wallets: 1,
      consensus_score: 0.5,
      recommended_action: 'SKIP',
      confidence: 0.5,
    },
  ],
  aggregation_latency_ms: 150,
}

const clusteringData: WalletClusteringResponse = {
  clusters: [
    {
      id: 'c1',
      wallets: ['w1', 'w2', 'w3'],
      signal_count: 8,
      avg_wqs: 72.5,
      last_activity: '2025-01-01T00:00:00Z',
      coherence: 0.9,
    },
  ],
  total_wallets: 3,
  clustering_metrics: {
    avg_cluster_size: 3,
    max_cluster_size: 3,
    silhouette_score: 0.7,
    modularity: 0.5,
  },
}

describe('consensus barrel', () => {
  it('re-exports all components', () => {
    expect(consensusBarrel.ConsensusOverview).toBeTruthy()
    expect(consensusBarrel.WalletClustersVisualization).toBeTruthy()
    expect(consensusBarrel.SignalAggregationView).toBeTruthy()
  })
})

describe('ConsensusOverview', () => {
  it('renders metrics and clusters', () => {
    render(<ConsensusOverview data={consensusData} />)
    expect(screen.getByText('Consensus Rate')).toBeInTheDocument()
    expect(screen.getByText('80.0%')).toBeInTheDocument()
    expect(screen.getByText(/Active Clusters \(2\)/)).toBeInTheDocument()
  })

  it('renders without active clusters', () => {
    const { container } = render(
      <ConsensusOverview data={{ ...consensusData, active_clusters: [] }} />
    )
    expect(container.textContent).not.toContain('Active Clusters')
  })
})

describe('SignalAggregationView', () => {
  it('renders the aggregation table', () => {
    render(<SignalAggregationView data={aggregationData} />)
    expect(screen.getByText('Total Signals')).toBeInTheDocument()
    expect(screen.getByText('$ABC')).toBeInTheDocument()
    expect(screen.getByText('$Unknown')).toBeInTheDocument()
    expect(screen.getByText('BUY')).toBeInTheDocument()
    expect(screen.getByText('HOLD')).toBeInTheDocument()
    expect(screen.getByText('SELL')).toBeInTheDocument()
    expect(screen.getByText('SKIP')).toBeInTheDocument()
    expect(screen.getByText('150ms')).toBeInTheDocument()
  })

  it('renders with empty aggregated signals', () => {
    const { container } = render(
      <SignalAggregationView data={{ ...aggregationData, aggregated_signals: [] }} />
    )
    expect(container.querySelector('tbody')?.children).toHaveLength(0)
  })
})

describe('WalletClustersVisualization', () => {
  it('renders clustering metrics and cluster rows', () => {
    render(<WalletClustersVisualization data={clusteringData} />)
    expect(screen.getByText('Total Wallets')).toBeInTheDocument()
    expect(screen.getByText('c1...')).toBeInTheDocument()
    expect(screen.getByText('0.90')).toBeInTheDocument()
  })

  it('renders without clusters', () => {
    const { container } = render(
      <WalletClustersVisualization data={{ clusters: [], total_wallets: 0, clustering_metrics: undefined as never }} />
    )
    expect(container.querySelector('tbody')?.children).toHaveLength(0)
  })
})
