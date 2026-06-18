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

// Fetch Signal Quality
export function useSignalQuality(timeRange?: string) {
  return useQuery({
    queryKey: ['signals', 'quality', timeRange],
    queryFn: async () => {
      const response = await apiClient.get<SignalQualityResponse>('/api/v1/signals/quality', {
        params: timeRange ? { range: timeRange } : undefined,
      })
      return response.data
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
      const response = await apiClient.get<SignalSourceResponse>('/api/v1/signals/sources')
      return response.data
    },
    staleTime: 60000,
  })
}

// Fetch Signal Consensus
export function useSignalConsensus() {
  return useQuery({
    queryKey: ['signals', 'consensus'],
    queryFn: async () => {
      const response = await apiClient.get<SignalConsensusResponse>('/api/v1/signals/consensus')
      return response.data
    },
    refetchInterval: 15000,
    staleTime: 5000,
  })
}
