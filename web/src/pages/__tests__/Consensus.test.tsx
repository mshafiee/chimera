import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen } from '@testing-library/react'
import { Consensus } from '../Consensus'

const apiMock = vi.hoisted(() => ({
  useConsensus: vi.fn(),
  useWalletClustering: vi.fn(),
  useConsensusSignalAggregation: vi.fn(),
}))

vi.mock('../../api', () => apiMock)

const consensusData = {
  consensus_rate: 0.8,
  avg_clustering_coefficient: 0.6,
  active_clusters: [
    { id: 'c1', wallets: ['w1'], signal_count: 5, avg_wqs: 70, last_activity: '2025-01-01T00:00:00Z', coherence: 0.9 },
  ],
  recent_signals: [
    {
      signal_id: 's1',
      timestamp: '2025-01-01T00:00:00Z',
      token_address: 'tok-1',
      token_symbol: 'T1',
      consensus_level: 'strong',
      wallet_count: 4,
      supporting_wallets: ['w1'],
      quality_score: 0.9,
      executed: true,
      execution_result: { success: true, pnl_sol: 0.5, execution_time_ms: 10 },
    },
    {
      signal_id: 's2',
      timestamp: '2025-01-02T00:00:00Z',
      token_address: 'tok-2',
      token_symbol: null,
      consensus_level: 'weak',
      wallet_count: 1,
      supporting_wallets: ['w2'],
      quality_score: 0.3,
      executed: false,
      execution_result: null,
    },
  ],
  divergence_alerts: [
    {
      alert_id: 'a1',
      timestamp: '2025-01-01T00:00:00Z',
      token_address: 'tok-3',
      token_symbol: 'T3',
      divergence_type: 'directional',
      severity: 'high',
      wallets_clustered: [{ cluster_id: 'x', wallet_addresses: ['w1'], signal: 'BUY' }],
      wallets_divergent: [{ cluster_id: 'y', wallet_addresses: ['w2'], signal: 'SELL' }],
    },
  ],
}

const clustering = {
  clusters: [{ id: 'c1', wallets: ['w1'], signal_count: 5, avg_wqs: 70, last_activity: '2025-01-01T00:00:00Z', coherence: 0.9 }],
  total_wallets: 1,
  clustering_metrics: { avg_cluster_size: 1, max_cluster_size: 1, silhouette_score: 0.7, modularity: 0.5 },
}

const aggregation = {
  window_start: 'x',
  window_end: 'y',
  total_signals: 5,
  unique_tokens: 2,
  aggregated_signals: [
    { token_address: 'tok-1', token_symbol: 'T1', signal_count: 3, unique_wallets: 2, consensus_score: 0.8, recommended_action: 'BUY', confidence: 0.9 },
  ],
  aggregation_latency_ms: 100,
}

function setup() {
  apiMock.useConsensus.mockReturnValue({ data: consensusData, isLoading: false })
  apiMock.useWalletClustering.mockReturnValue({ data: clustering, isLoading: false })
  apiMock.useConsensusSignalAggregation.mockReturnValue({ data: aggregation, isLoading: false })
}

beforeEach(() => {
  vi.clearAllMocks()
  setup()
})

describe('Consensus', () => {
  it('renders the full consensus page', () => {
    render(<Consensus />)
    expect(screen.getByText('Signal Consensus')).toBeInTheDocument()
    expect(screen.getByText('Consensus Overview')).toBeInTheDocument()
    expect(screen.getByText('Signal Aggregation')).toBeInTheDocument()
    expect(screen.getByText('Wallet Clustering')).toBeInTheDocument()
    expect(screen.getByText('Recent Consensus Signals')).toBeInTheDocument()
    expect(screen.getAllByText('Divergence Alerts').length).toBeGreaterThan(0)
    expect(screen.getAllByText('$T1').length).toBeGreaterThan(0)
    expect(screen.getByText('$T3')).toBeInTheDocument()
    expect(screen.getByText('+0.5000 SOL')).toBeInTheDocument()
    expect(screen.getByText('strong')).toBeInTheDocument()
    expect(screen.getByText('weak')).toBeInTheDocument()
    expect(screen.getByText('directional')).toBeInTheDocument()
  })

  it('renders loading states', () => {
    apiMock.useConsensus.mockReturnValue({ data: undefined, isLoading: true })
    apiMock.useWalletClustering.mockReturnValue({ data: undefined, isLoading: true })
    apiMock.useConsensusSignalAggregation.mockReturnValue({ data: undefined, isLoading: true })
    render(<Consensus />)
    expect(screen.getByText('Loading consensus data...')).toBeInTheDocument()
    expect(screen.getByText('Loading aggregation data...')).toBeInTheDocument()
    expect(screen.getByText('Loading clustering data...')).toBeInTheDocument()
  })

  it('renders empty states', () => {
    apiMock.useConsensus.mockReturnValue({ data: undefined, isLoading: false })
    apiMock.useWalletClustering.mockReturnValue({ data: undefined, isLoading: false })
    apiMock.useConsensusSignalAggregation.mockReturnValue({ data: undefined, isLoading: false })
    render(<Consensus />)
    expect(screen.getByText('No consensus data available')).toBeInTheDocument()
    expect(screen.getByText('No aggregation data available')).toBeInTheDocument()
    expect(screen.getByText('No clustering data available')).toBeInTheDocument()
  })

  it('renders failed executions and no divergence alerts', () => {
    apiMock.useConsensus.mockReturnValue({
      data: {
        ...consensusData,
        recent_signals: [
          {
            ...consensusData.recent_signals[0],
            executed: true,
            execution_result: { success: false },
          },
          {
            ...consensusData.recent_signals[0],
            consensus_level: 'moderate',
            quality_score: 0.5,
            execution_result: { success: true, pnl_sol: null, execution_time_ms: 5 },
          },
        ],
        divergence_alerts: [],
      },
      isLoading: false,
    })
    render(<Consensus />)
    expect(screen.getByText('Failed')).toBeInTheDocument()
    expect(screen.getByText('+N/A SOL')).toBeInTheDocument()
    expect(screen.queryByText('directional')).not.toBeInTheDocument()
  })
})
