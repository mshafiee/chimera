import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../ui/Table'
import { Badge } from '../ui/Badge'
import type { WalletClusteringResponse } from '../../api'

interface WalletClustersVisualizationProps {
  data: WalletClusteringResponse
}

export function WalletClustersVisualization({ data }: WalletClustersVisualizationProps) {
  return (
    <div className="space-y-6">
      {/* Clustering Metrics */}
      <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Total Wallets</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.total_wallets}
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Clusters</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.clusters.length}
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Avg Cluster Size</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.clustering_metrics.avg_cluster_size.toFixed(1)}
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Silhouette Score</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.clustering_metrics.silhouette_score.toFixed(2)}
          </div>
        </div>
      </div>

      {/* Clusters Table */}
      <Table>
        <TableHeader>
          <TableRow hoverable={false}>
            <TableHead>Cluster ID</TableHead>
            <TableHead className="text-right">Wallets</TableHead>
            <TableHead className="text-right">Signals</TableHead>
            <TableHead className="text-right">Avg WQS</TableHead>
            <TableHead>Coherence</TableHead>
            <TableHead>Last Activity</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {data.clusters.map((cluster) => (
            <TableRow key={cluster.id}>
              <TableCell mono className="text-sm">
                {cluster.id.slice(0, 12)}...
              </TableCell>
              <TableCell mono className="text-sm text-right">
                {cluster.wallets.length}
              </TableCell>
              <TableCell mono className="text-sm text-right">
                {cluster.signal_count}
              </TableCell>
              <TableCell mono className="text-sm text-right">
                <span className={cluster.avg_wqs >= 60 ? 'text-profit' : cluster.avg_wqs >= 40 ? 'text-spear' : 'text-loss'}>
                  {cluster.avg_wqs.toFixed(1)}
                </span>
              </TableCell>
              <TableCell>
                <Badge
                  variant={cluster.coherence >= 0.7 ? 'success' : cluster.coherence >= 0.5 ? 'warning' : 'default'}
                  size="sm"
                >
                  {cluster.coherence.toFixed(2)}
                </Badge>
              </TableCell>
              <TableCell className="text-sm text-text-muted">
                {new Date(cluster.last_activity).toLocaleString()}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  )
}
