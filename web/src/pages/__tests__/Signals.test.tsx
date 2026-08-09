import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { Signals } from '../Signals'

vi.mock('recharts', async () => await import('../../test-utils/rechartsMock'))

const apiMock = vi.hoisted(() => ({
  useSignalQuality: vi.fn(),
  useSignalSources: vi.fn(),
  useSignalConsensus: vi.fn(),
  useConsensus: vi.fn(),
}))

vi.mock('../../api', () => apiMock)

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
  sources: [{ source: 'source-address-1', signal_count: 45, average_quality: 0.68, acceptance_rate: 0.75, last_signal_at: '2025-01-01T00:00:00Z' }],
  total_signals: 45,
}

const consensus = {
  consensus_detection_rate: 0.8,
  average_clustering: 0.68,
  divergence_alerts: [
    { timestamp: '2025-01-01T00:00:00Z', token_address: 'tok-2', token_symbol: 'T2', divergence_score: 0.75, wallets_divergent: ['w1', 'w2'] },
  ],
  consensus_signals: [
    { timestamp: '2025-01-01T00:00:00Z', token_address: 'tok-1', token_symbol: 'T1', consensus_wallets: 3, total_wallets: 5, quality_score: 0.82 },
  ],
}

const consensusData = {
  consensus_rate: 0.5,
  avg_clustering_coefficient: 0.5,
  active_clusters: [
    { id: 'c1', wallets: ['w1', 'w2'], signal_count: 4, avg_wqs: 70, last_activity: '2025-01-01T00:00:00Z', coherence: 0.8 },
  ],
  recent_signals: [],
  divergence_alerts: [],
}

function setup() {
  apiMock.useSignalQuality.mockReturnValue({ data: quality, isLoading: false })
  apiMock.useSignalSources.mockReturnValue({ data: sources, isLoading: false })
  apiMock.useSignalConsensus.mockReturnValue({ data: consensus, isLoading: false })
  apiMock.useConsensus.mockReturnValue({ data: consensusData, isLoading: false })
}

beforeEach(() => {
  vi.clearAllMocks()
  setup()
})

describe('Signals', () => {
  it('renders the full signals page', () => {
    render(<Signals />)
    expect(screen.getByText('Signal Intelligence')).toBeInTheDocument()
    expect(screen.getByText('Current Quality Score')).toBeInTheDocument()
    expect(screen.getByText(/0\.80/)).toBeInTheDocument()
    expect(screen.getByText('Signal Quality Distribution')).toBeInTheDocument()
    expect(screen.getByText('Signal Sources')).toBeInTheDocument()
    expect(screen.getByText('Consensus Matrix')).toBeInTheDocument()
    expect(screen.getByText('Signal Consensus')).toBeInTheDocument()
    expect(screen.getAllByText('80.0%').length).toBeGreaterThan(0)
    expect(screen.getByText('$T1')).toBeInTheDocument()
    expect(screen.getByText('$T2')).toBeInTheDocument()
  })

  it('renders loading and empty states', () => {
    apiMock.useSignalQuality.mockReturnValue({ data: undefined, isLoading: true })
    apiMock.useSignalSources.mockReturnValue({ data: undefined, isLoading: true })
    apiMock.useConsensus.mockReturnValue({ data: undefined, isLoading: true })
    apiMock.useSignalConsensus.mockReturnValue({ data: undefined, isLoading: true })
    render(<Signals />)
    expect(screen.getByText('Loading signal quality...')).toBeInTheDocument()
    expect(screen.getByText('Loading signal sources...')).toBeInTheDocument()
    expect(screen.getByText('Loading consensus matrix...')).toBeInTheDocument()
    expect(screen.getByText('Loading consensus data...')).toBeInTheDocument()
  })

  it('renders without data', () => {
    apiMock.useSignalQuality.mockReturnValue({ data: undefined, isLoading: false })
    apiMock.useSignalSources.mockReturnValue({ data: undefined, isLoading: false })
    apiMock.useConsensus.mockReturnValue({ data: undefined, isLoading: false })
    apiMock.useSignalConsensus.mockReturnValue({ data: undefined, isLoading: false })
    render(<Signals />)
    expect(screen.getByText('No signal quality data available')).toBeInTheDocument()
    expect(screen.getByText('No signal sources available')).toBeInTheDocument()
    expect(screen.getAllByText('No consensus data available').length).toBeGreaterThan(0)
  })

  it('renders consensus signals with low detection rate and empty divergence', () => {
    apiMock.useSignalConsensus.mockReturnValue({
      data: { ...consensus, consensus_detection_rate: 0.2, divergence_alerts: [], consensus_signals: [] },
      isLoading: false,
    })
    render(<Signals />)
    expect(screen.getAllByText('20.0%').length).toBeGreaterThan(0)
  })

  it('changes the time range', () => {
    render(<Signals />)
    fireEvent.click(screen.getByText('7D'))
    expect(apiMock.useSignalQuality).toHaveBeenCalled()
  })
})
