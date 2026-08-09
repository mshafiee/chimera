import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { SignalsDashboard } from '../SignalsDashboard'

vi.mock('recharts', async () => await import('../../test-utils/rechartsMock'))

const signalsApiMock = vi.hoisted(() => ({
  useSignalQuality: vi.fn(),
  useSignalSources: vi.fn(),
  useSignalConsensus: vi.fn(),
  useSignalAggregation: vi.fn(),
  useSignalClustering: vi.fn(),
}))

vi.mock('../../api/signals', () => signalsApiMock)

const useWebSocketMock = vi.hoisted(() => vi.fn())
const useDashboardWebSocketMock = vi.hoisted(() => vi.fn())

vi.mock('../../hooks/useWebSocket', () => ({ useWebSocket: useWebSocketMock }))
vi.mock('../../hooks/useDashboardWebSocket', () => ({
  useDashboardWebSocket: useDashboardWebSocketMock,
  DASHBOARD_UPDATE_EVENT: 'dashboard:update',
}))

const quality = {
  current_quality_score: 0.8,
  quality_distribution: [{ range: '0.8-1.0', count: 30, percentage: 30 }],
  rejection_rate: 0.1,
  total_signals: 100,
  accepted_signals: 90,
  rejected_signals: 10,
  average_quality_trend: [{ timestamp: '2025-01-01T00:00:00Z', average_score: 0.7 }],
}

const sources = {
  sources: [
    { source: 'source-address-1', signal_count: 45, average_quality: 0.68, acceptance_rate: 0.75, last_signal_at: '2025-01-01T00:00:00Z' },
    { source: 'source-address-2', signal_count: 10, average_quality: 0.9, acceptance_rate: 0.9, last_signal_at: '2025-01-02T00:00:00Z' },
  ],
  total_signals: 55,
}

const consensus = {
  consensus_detection_rate: 0.8,
  average_clustering: 0.68,
  divergence_alerts: [{ timestamp: '2025-01-01T00:00:00Z', token_address: 'tok-2', token_symbol: 'T2', divergence_score: 0.75, wallets_divergent: ['w1'] }],
  consensus_signals: [{ timestamp: '2025-01-01T00:00:00Z', token_address: 'tok-1', token_symbol: 'T1', consensus_wallets: 4, total_wallets: 5, quality_score: 0.82 }],
}

const aggregation = {
  total_aggregated_windows: 144,
  average_signals_per_window: 8.5,
  aggregation_trend: [{ timestamp: '2025-01-01T00:00:00Z', signal_count: 10, window_count: 2 }],
  top_aggregated_tokens: [
    { token_address: 'tok-agg-1', token_symbol: 'AGG1', aggregated_signal_count: 25, unique_wallets: 8, average_quality_score: 0.76 },
    { token_address: 'tok-agg-2', token_symbol: null, aggregated_signal_count: 20, unique_wallets: 6, average_quality_score: 0.71 },
  ],
}

const clustering = {
  total_clusters: 5,
  average_cluster_size: 3.2,
  largest_cluster_size: 6,
  clustering_coefficient: 0.68,
  clusters: [
    { cluster_id: 1, size: 4, wallet_addresses: ['w1', 'w2', 'w3', 'w4', 'w5', 'w6', 'w7'], common_tokens: ['tok-a', 'tok-b'], average_quality: 0.74, consensus_rate: 0.82 },
  ],
}

function setup(loading = false) {
  signalsApiMock.useSignalQuality.mockReturnValue({ data: quality, isLoading: loading })
  signalsApiMock.useSignalSources.mockReturnValue({ data: sources, isLoading: loading })
  signalsApiMock.useSignalConsensus.mockReturnValue({ data: consensus, isLoading: loading })
  signalsApiMock.useSignalAggregation.mockReturnValue({ data: aggregation, isLoading: loading })
  signalsApiMock.useSignalClustering.mockReturnValue({ data: clustering, isLoading: loading })
  useWebSocketMock.mockReturnValue({ isConnected: true, isConnecting: false, connectionError: null })
  useDashboardWebSocketMock.mockImplementation((opts: {
    onSignalUpdate?: (d: unknown) => void
    onConsensusAlert?: (d: unknown) => void
    onQualityChange?: (d: unknown) => void
  }) => {
    opts.onSignalUpdate?.({ message: 'sig' })
    opts.onConsensusAlert?.({ message: 'con' })
    opts.onQualityChange?.({ message: 'q' })
    return { refreshSignalData: vi.fn() }
  })
}

beforeEach(() => {
  vi.clearAllMocks()
  setup()
})

describe('SignalsDashboard', () => {
  it('renders the loading state', () => {
    setup(true)
    render(<SignalsDashboard />)
    expect(screen.getByText('Loading signal analysis...')).toBeInTheDocument()
  })

  it('renders the full dashboard', () => {
    render(<SignalsDashboard />)
    expect(screen.getByText('Signal Analysis Dashboard')).toBeInTheDocument()
    expect(screen.getByText('Top Signal Sources')).toBeInTheDocument()
    expect(screen.getByText('Source Performance')).toBeInTheDocument()
    expect(screen.getByText('Acceptance Rates')).toBeInTheDocument()
    expect(screen.getByText('Aggregated Windows')).toBeInTheDocument()
    expect(screen.getByText('Top Tokens')).toBeInTheDocument()
    expect(screen.getByText('Signal Aggregation Trend')).toBeInTheDocument()
    expect(screen.getByText('Top Aggregated Tokens')).toBeInTheDocument()
    expect(screen.getByText('Total Clusters')).toBeInTheDocument()
    expect(screen.getByText('Wallet Clusters')).toBeInTheDocument()
    expect(screen.getByText('AGG1')).toBeInTheDocument()
    expect(screen.getByText('Unknown')).toBeInTheDocument()
    expect(screen.getAllByText('#1').length).toBeGreaterThan(0)
    expect(screen.getByText('+1 more')).toBeInTheDocument()
  })

  it('renders without sources, aggregation trend and clusters', () => {
    signalsApiMock.useSignalSources.mockReturnValue({ data: { sources: [], total_signals: 0 }, isLoading: false })
    signalsApiMock.useSignalAggregation.mockReturnValue({
      data: { ...aggregation, aggregation_trend: [], top_aggregated_tokens: [] },
      isLoading: false,
    })
    signalsApiMock.useSignalClustering.mockReturnValue({
      data: { ...clustering, clusters: [] },
      isLoading: false,
    })
    render(<SignalsDashboard />)
    expect(screen.queryByText('Top Signal Sources')).not.toBeInTheDocument()
    expect(screen.queryByText('Signal Aggregation Trend')).not.toBeInTheDocument()
    expect(screen.queryByText('Wallet Clusters')).not.toBeInTheDocument()
  })

  it('refreshes signal data and changes the time range', () => {
    const refreshSignalData = vi.fn()
    useDashboardWebSocketMock.mockReturnValue({ refreshSignalData })
    render(<SignalsDashboard />)
    fireEvent.click(screen.getByRole('button', { name: /refresh data/i }))
    expect(refreshSignalData).toHaveBeenCalled()
    fireEvent.click(screen.getByText('7D'))
    expect(signalsApiMock.useSignalQuality).toHaveBeenCalled()
  })
})
