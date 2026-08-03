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
  signal: 'BUY' | 'SELL'
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

const CONSENSUS_ENDPOINT = '/signals/consensus'
const CLUSTERING_ENDPOINT = '/signals/clustering'
const AGGREGATION_ENDPOINT = '/signals/aggregation'

function useConsensusPolling<T>(
  queryKey: unknown[],
  url: string,
  refetchInterval: number,
  staleTime: number,
  label: string
) {
  return useQuery({
    queryKey,
    queryFn: async ({ signal }) => {
      const response = await apiClient.get<T>(url, { signal })
      return response.data
    },
    refetchInterval,
    staleTime,
    retry: 1,
    meta: {
      onError: (error: unknown) => {
        console.error(`[Consensus API] Failed to fetch ${label}:`, error)
      },
    },
  })
}

// Fetch Consensus Data
export function useConsensus() {
  return useConsensusPolling<ConsensusResponse>(['consensus'], CONSENSUS_ENDPOINT, 15000, 5000, 'consensus data')
}

// Fetch Wallet Clustering
export function useWalletClustering() {
  return useConsensusPolling<WalletClusteringResponse>(['consensus', 'clustering'], CLUSTERING_ENDPOINT, 60000, 30000, 'wallet clustering')
}

// Fetch Signal Aggregation
export function useSignalAggregation() {
  return useConsensusPolling<SignalAggregationResponse>(['consensus', 'aggregation'], AGGREGATION_ENDPOINT, 10000, 5000, 'signal aggregation')
}
