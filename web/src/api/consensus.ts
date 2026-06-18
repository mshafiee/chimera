import { useQuery } from '@tanstack/react-query'
import { apiClient } from './client'

// Signal Consensus Response (Enhanced)
export interface ConsensusResponse {
  consensus_rate: number
  avg_clustering_coefficient: number
  active_clusters: Cluster[]
  recent_signals: ConsensusSignal[]
  divergence_alerts: DivergenceAlert[]
}

export interface Cluster {
  id: string
  wallets: string[]
  signal_count: number
  avg_wqs: number
  last_activity: string
  coherence: number
}

export interface ConsensusSignal {
  signal_id: string
  timestamp: string
  token_address: string
  token_symbol: string | null
  consensus_level: 'strong' | 'moderate' | 'weak' | 'none'
  wallet_count: number
  supporting_wallets: string[]
  quality_score: number
  executed: boolean
  execution_result: {
    success: boolean
    pnl_sol?: number
    execution_time_ms?: number
  } | null
}

export interface DivergenceAlert {
  alert_id: string
  timestamp: string
  token_address: string
  token_symbol: string | null
  divergence_type: 'directional' | 'timing' | 'amount'
  severity: 'low' | 'medium' | 'high'
  wallets_clustered: WalletCluster[]
  wallets_divergent: WalletCluster[]
}

export interface WalletCluster {
  cluster_id: string
  wallet_addresses: string[]
  signal: string // 'BUY' or 'SELL'
}

// Wallet Clustering Analysis
export interface WalletClusteringResponse {
  clusters: Cluster[]
  total_wallets: number
  clustering_metrics: ClusteringMetrics
}

export interface ClusteringMetrics {
  avg_cluster_size: number
  max_cluster_size: number
  silhouette_score: number
  modularity: number
}

// Signal Aggregation Status
export interface SignalAggregationResponse {
  window_start: string
  window_end: string
  total_signals: number
  unique_tokens: number
  aggregated_signals: AggregatedSignal[]
  aggregation_latency_ms: number
}

export interface AggregatedSignal {
  token_address: string
  token_symbol: string | null
  signal_count: number
  unique_wallets: number
  consensus_score: number
  recommended_action: 'BUY' | 'SELL' | 'HOLD' | 'SKIP'
  confidence: number
}

// Mock data for when API is not available
const mockConsensus: ConsensusResponse = {
  consensus_rate: 0,
  avg_clustering_coefficient: 0,
  active_clusters: [],
  recent_signals: [],
  divergence_alerts: []
}

const mockWalletClustering: WalletClusteringResponse = {
  clusters: [],
  total_wallets: 0,
  clustering_metrics: {
    avg_cluster_size: 0,
    max_cluster_size: 0,
    silhouette_score: 0,
    modularity: 0
  }
}

const mockSignalAggregation: SignalAggregationResponse = {
  window_start: new Date().toISOString(),
  window_end: new Date().toISOString(),
  total_signals: 0,
  unique_tokens: 0,
  aggregated_signals: [],
  aggregation_latency_ms: 0
}

// Fetch Consensus Data
export function useConsensus() {
  return useQuery({
    queryKey: ['consensus'],
    queryFn: async () => {
      try {
        const response = await apiClient.get<ConsensusResponse>('/signals/consensus')
        return response.data
      } catch (error: any) {
        if (error.response?.status === 404) {
          console.warn('[Consensus API] Consensus endpoint not implemented, using mock data')
          return mockConsensus
        }
        throw error
      }
    },
    refetchInterval: 15000,
    staleTime: 5000,
  })
}

// Fetch Wallet Clustering
export function useWalletClustering() {
  return useQuery({
    queryKey: ['consensus', 'clustering'],
    queryFn: async () => {
      try {
        const response = await apiClient.get<WalletClusteringResponse>('/signals/clustering')
        return response.data
      } catch (error: any) {
        if (error.response?.status === 404) {
          console.warn('[Consensus API] Clustering endpoint not implemented, using mock data')
          return mockWalletClustering
        }
        throw error
      }
    },
    refetchInterval: 60000,
    staleTime: 30000,
  })
}

// Fetch Signal Aggregation
export function useSignalAggregation() {
  return useQuery({
    queryKey: ['consensus', 'aggregation'],
    queryFn: async () => {
      try {
        const response = await apiClient.get<SignalAggregationResponse>('/signals/aggregation')
        return response.data
      } catch (error: any) {
        if (error.response?.status === 404) {
          console.warn('[Consensus API] Aggregation endpoint not implemented, using mock data')
          return mockSignalAggregation
        }
        throw error
      }
    },
    refetchInterval: 10000,
    staleTime: 5000,
  })
}
