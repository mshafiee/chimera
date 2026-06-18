import { useQuery } from '@tanstack/react-query'
import { apiClient } from './client'

// Signal Quality Response
export interface SignalQualityResponse {
  current_quality_score: number
  quality_distribution: QualityBucket[]
  rejection_rate: number
  total_signals: number
  accepted_signals: number
  rejected_signals: number
  average_quality_trend: QualityTrendPoint[]
}

export interface QualityBucket {
  range: string // e.g., "0-0.2", "0.2-0.4", etc.
  count: number
  percentage: number
}

export interface QualityTrendPoint {
  timestamp: string
  average_score: number
}

// Signal Source Breakdown
export interface SignalSourceResponse {
  sources: SignalSource[]
  total_signals: number
}

export interface SignalSource {
  source: string // wallet address or source identifier
  signal_count: number
  average_quality: number
  acceptance_rate: number
  last_signal_at: string
}

// Signal Consensus
export interface SignalConsensusResponse {
  consensus_detection_rate: number
  average_clustering: number
  divergence_alerts: DivergenceAlert[]
  consensus_signals: ConsensusSignal[]
}

export interface DivergenceAlert {
  timestamp: string
  token_address: string
  token_symbol: string | null
  divergence_score: number
  wallets_divergent: string[]
}

export interface ConsensusSignal {
  timestamp: string
  token_address: string
  token_symbol: string | null
  consensus_wallets: number
  total_wallets: number
  quality_score: number
}

// Mock data for when API is not available
const mockSignalQuality: SignalQualityResponse = {
  current_quality_score: 0,
  quality_distribution: [
    { range: '0-0.2', count: 0, percentage: 0 },
    { range: '0.2-0.4', count: 0, percentage: 0 },
    { range: '0.4-0.6', count: 0, percentage: 0 },
    { range: '0.6-0.8', count: 0, percentage: 0 },
    { range: '0.8-1.0', count: 0, percentage: 0 }
  ],
  rejection_rate: 0,
  total_signals: 0,
  accepted_signals: 0,
  rejected_signals: 0,
  average_quality_trend: []
}

const mockSignalSources: SignalSourceResponse = {
  sources: [],
  total_signals: 0
}

const mockSignalConsensus: SignalConsensusResponse = {
  consensus_detection_rate: 0,
  average_clustering: 0,
  divergence_alerts: [],
  consensus_signals: []
}

// Fetch Signal Quality
export function useSignalQuality(timeRange?: string) {
  return useQuery({
    queryKey: ['signals', 'quality', timeRange],
    queryFn: async () => {
      try {
        const response = await apiClient.get<SignalQualityResponse>('/signals/quality', {
          params: timeRange ? { range: timeRange } : undefined,
        })
        return response.data
      } catch (error: any) {
        if (error.response?.status === 404) {
          console.warn('[Signals API] Quality endpoint not implemented, using mock data')
          return mockSignalQuality
        }
        throw error
      }
    },
    refetchInterval: 30000,
    staleTime: 10000,
  })
}

// Fetch Signal Sources
export function useSignalSources() {
  return useQuery({
    queryKey: ['signals', 'sources'],
    queryFn: async () => {
      try {
        const response = await apiClient.get<SignalSourceResponse>('/signals/sources')
        return response.data
      } catch (error: any) {
        if (error.response?.status === 404) {
          console.warn('[Signals API] Sources endpoint not implemented, using mock data')
          return mockSignalSources
        }
        throw error
      }
    },
    staleTime: 60000,
  })
}

// Fetch Signal Consensus
export function useSignalConsensus() {
  return useQuery({
    queryKey: ['signals', 'consensus'],
    queryFn: async () => {
      try {
        const response = await apiClient.get<SignalConsensusResponse>('/signals/consensus')
        return response.data
      } catch (error: any) {
        if (error.response?.status === 404) {
          console.warn('[Signals API] Consensus endpoint not implemented, using mock data')
          return mockSignalConsensus
        }
        throw error
      }
    },
    refetchInterval: 15000,
    staleTime: 5000,
  })
}
