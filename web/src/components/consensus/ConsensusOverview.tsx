import { MetricCard } from '../ui/MetricCard'
import type { ConsensusResponse } from '../../api'
import { Network, Users, AlertTriangle } from 'lucide-react'

interface ConsensusOverviewProps {
  data: ConsensusResponse
}

export function ConsensusOverview({ data }: ConsensusOverviewProps) {
  return (
    <div className="space-y-6">
      {/* Summary Metrics */}
      <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
        <MetricCard
          label="Consensus Rate"
          value={`${(data.consensus_rate * 100).toFixed(1)}%`}
          positive={data.consensus_rate > 0.7}
          icon={<Network className="w-4 h-4" />}
        />
        <MetricCard
          label="Avg Clustering"
          value={data.avg_clustering_coefficient.toFixed(2)}
          icon={<Users className="w-4 h-4" />}
        />
        <MetricCard
          label="Divergence Alerts"
          value={data.divergence_alerts.length}
          positive={data.divergence_alerts.length === 0}
          icon={<AlertTriangle className="w-4 h-4" />}
        />
      </div>

      {/* Active Clusters Summary */}
      {data.active_clusters.length > 0 && (
        <div>
          <h3 className="text-sm font-medium mb-3">Active Clusters ({data.active_clusters.length})</h3>
          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-3">
            {data.active_clusters.slice(0, 6).map((cluster) => (
              <div key={cluster.id} className="bg-surface-light rounded-lg p-3">
                <div className="flex items-center justify-between mb-2">
                  <span className="text-xs text-text-muted">Cluster</span>
                  <span className="text-xs font-mono-numbers">{cluster.id.slice(0, 8)}</span>
                </div>
                <div className="flex items-center justify-between text-sm">
                  <span>{cluster.wallets.length} wallets</span>
                  <span className="text-text-muted">Coherence: {cluster.coherence.toFixed(2)}</span>
                </div>
                <div className="text-xs text-text-muted mt-1">
                  Avg WQS: {cluster.avg_wqs.toFixed(1)}
                </div>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  )
}
