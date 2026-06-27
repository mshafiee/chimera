import { useQuery } from '@tanstack/react-query'
import { toast } from 'sonner'
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

// Signal Aggregation
export interface SignalAggregationResponse {
  total_aggregated_windows: number
  average_signals_per_window: number
  aggregation_trend: AggregationTrendPoint[]
  top_aggregated_tokens: AggregatedToken[]
}

export interface AggregationTrendPoint {
  timestamp: string
  signal_count: number
  window_count: number
}

export interface AggregatedToken {
  token_address: string
  token_symbol: string | null
  aggregated_signal_count: number
  unique_wallets: number
  average_quality_score: number
}

// Signal Clustering
export interface SignalClusteringResponse {
  total_clusters: number
  average_cluster_size: number
  largest_cluster_size: number
  clustering_coefficient: number
  clusters: Cluster[]
}

export interface Cluster {
  cluster_id: number
  size: number
  wallet_addresses: string[]
  common_tokens: string[]
  average_quality: number
  consensus_rate: number
}

// Fetch Signal Quality
export function useSignalQuality(timeRange?: string) {
  return useQuery({
    queryKey: ['signals', 'quality', timeRange],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<SignalQualityResponse>('/signals/quality', {
        params: timeRange ? { range: timeRange } : undefined,
        signal,
      })
      return response.data
    },
    refetchInterval: 30000,
    staleTime: 10000,
    retry: 3,
    meta: {
      onError: (error: unknown) => {
        console.error('[Signals API] Failed to fetch signal quality:', error)
        // Signal quality is critical - show toast notification
        toast.error('Failed to load signal quality. Please try again later.')
      },
    },
  })
}

// Fetch Signal Sources
export function useSignalSources() {
  return useQuery({
    queryKey: ['signals', 'sources'],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<SignalSourceResponse>('/signals/sources', { signal })
      return response.data
    },
    staleTime: 60000,
    retry: 1,
    meta: {
      onError: (error: unknown) => {
        console.error('[Signals API] Failed to fetch signal sources:', error)
        // Signal sources are optional - console only
      },
    },
  })
}

// Fetch Signal Consensus
export function useSignalConsensus() {
  return useQuery({
    queryKey: ['signals', 'consensus'],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<SignalConsensusResponse>('/signals/consensus', { signal })
      return response.data
    },
    refetchInterval: 15000,
    staleTime: 5000,
    retry: 1,
    meta: {
      onError: (error: unknown) => {
        console.error('[Signals API] Failed to fetch signal consensus:', error)
        // Consensus is optional - console only
      },
    },
  })
}

// Fetch Signal Aggregation
export function useSignalAggregation(timeRange?: string) {
  return useQuery({
    queryKey: ['signals', 'aggregation', timeRange],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<SignalAggregationResponse>('/signals/aggregation', {
        params: timeRange ? { range: timeRange } : undefined,
        signal,
      })
      return response.data
    },
    refetchInterval: 20000,
    staleTime: 10000,
    retry: 1,
    meta: {
      onError: (error: unknown) => {
        console.error('[Signals API] Failed to fetch signal aggregation:', error)
        // Aggregation is optional - console only
      },
    },
  })
}

// Fetch Signal Clustering
export function useSignalClustering() {
  return useQuery({
    queryKey: ['signals', 'clustering'],
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<SignalClusteringResponse>('/signals/clustering', { signal })
      return response.data
    },
    refetchInterval: 30000,
    staleTime: 15000,
    retry: 1,
    meta: {
      onError: (error: unknown) => {
        console.error('[Signals API] Failed to fetch signal clustering:', error)
        // Clustering is optional - console only
      },
    },
  })
}
