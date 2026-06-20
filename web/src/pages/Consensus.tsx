import { Card, CardHeader, CardTitle, CardContent } from '../components/ui/Card'
import { Badge } from '../components/ui/Badge'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../components/ui/Table'
import { useConsensus, useWalletClustering, useSignalAggregation } from '../api'
import { ConsensusOverview } from '../components/consensus/ConsensusOverview'
import { WalletClustersVisualization } from '../components/consensus/WalletClustersVisualization'
import { SignalAggregationView } from '../components/consensus/SignalAggregationView'

export function Consensus() {
  const { data: consensusData, isLoading: consensusLoading } = useConsensus()
  const { data: clusteringData, isLoading: clusteringLoading } = useWalletClustering()
  const { data: aggregationData, isLoading: aggregationLoading } = useSignalAggregation()

  return (
    <div className="space-y-6">
      {/* Header */}
      <div>
        <h1 className="text-2xl font-bold">Signal Consensus</h1>
        <p className="text-text-muted text-sm">Multi-wallet signal clustering and consensus analysis</p>
      </div>

      {/* Consensus Overview */}
      <Card>
        <CardHeader>
          <CardTitle>Consensus Overview</CardTitle>
        </CardHeader>
        <CardContent>
          {consensusLoading ? (
            <div className="text-center text-text-muted py-8">Loading consensus data...</div>
          ) : consensusData ? (
            <ConsensusOverview data={consensusData} />
          ) : (
            <div className="text-center text-text-muted py-8">No consensus data available</div>
          )}
        </CardContent>
      </Card>

      {/* Signal Aggregation */}
      <Card>
        <CardHeader>
          <CardTitle>Signal Aggregation</CardTitle>
        </CardHeader>
        <CardContent>
          {aggregationLoading ? (
            <div className="text-center text-text-muted py-8">Loading aggregation data...</div>
          ) : aggregationData ? (
            <SignalAggregationView data={aggregationData} />
          ) : (
            <div className="text-center text-text-muted py-8">No aggregation data available</div>
          )}
        </CardContent>
      </Card>

      {/* Wallet Clustering */}
      <Card>
        <CardHeader>
          <CardTitle>Wallet Clustering</CardTitle>
        </CardHeader>
        <CardContent>
          {clusteringLoading ? (
            <div className="text-center text-text-muted py-8">Loading clustering data...</div>
          ) : clusteringData ? (
            <WalletClustersVisualization data={clusteringData} />
          ) : (
            <div className="text-center text-text-muted py-8">No clustering data available</div>
          )}
        </CardContent>
      </Card>

      {/* Recent Consensus Signals */}
      {consensusData?.recent_signals?.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle>Recent Consensus Signals</CardTitle>
          </CardHeader>
          <CardContent>
            <Table>
              <TableHeader>
                <TableRow hoverable={false}>
                  <TableHead>Time</TableHead>
                  <TableHead>Token</TableHead>
                  <TableHead>Consensus Level</TableHead>
                  <TableHead>Wallets</TableHead>
                  <TableHead>Quality Score</TableHead>
                  <TableHead>Executed</TableHead>
                  <TableHead>Result</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {consensusData.recent_signals.slice(0, 20).map((signal) => (
                  <TableRow key={signal.signal_id}>
                    <TableCell className="text-sm text-text-muted">
                      {new Date(signal.timestamp).toLocaleString()}
                    </TableCell>
                    <TableCell>
                      <div className="font-semibold">
                        ${signal.token_symbol || 'Unknown'}
                      </div>
                      <div className="text-xs text-text-muted">
                        {signal.token_address.slice(0, 8)}...
                      </div>
                    </TableCell>
                    <TableCell>
                      <Badge
                        variant={
                          signal.consensus_level === 'strong' ? 'success' :
                          signal.consensus_level === 'moderate' ? 'warning' : 'default'
                        }
                        size="sm"
                      >
                        {signal.consensus_level}
                      </Badge>
                    </TableCell>
                    <TableCell mono className="text-sm">
                      {signal.wallet_count}
                    </TableCell>
                    <TableCell mono className="text-sm">
                      <span className={signal.quality_score >= 0.7 ? 'text-profit' : signal.quality_score >= 0.5 ? 'text-spear' : 'text-loss'}>
                        {signal.quality_score.toFixed(2)}
                      </span>
                    </TableCell>
                    <TableCell>
                      <Badge variant={signal.executed ? 'success' : 'default'} size="sm">
                        {signal.executed ? 'Yes' : 'No'}
                      </Badge>
                    </TableCell>
                    <TableCell>
                      {signal.execution_result ? (
                        <span className={signal.execution_result.success ? 'text-profit' : 'text-loss'}>
                          {signal.execution_result.success ? `+${signal.execution_result.pnl_sol?.toFixed(4) || 'N/A'} SOL` : 'Failed'}
                        </span>
                      ) : (
                        <span className="text-text-muted">—</span>
                      )}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </CardContent>
        </Card>
      )}

      {/* Divergence Alerts */}
      {consensusData?.divergence_alerts?.length > 0 && (
        <Card className="border-loss">
          <CardHeader>
            <CardTitle className="text-loss">Divergence Alerts</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="space-y-3">
              {consensusData.divergence_alerts.map((alert) => (
                <div key={alert.alert_id} className="bg-loss/10 border border-loss/30 rounded-lg p-4">
                  <div className="flex items-start justify-between">
                    <div className="flex-1">
                      <div className="flex items-center gap-3">
                        <div className="font-semibold">
                          ${alert.token_symbol || 'Unknown'}
                        </div>
                        <Badge
                          variant={alert.severity === 'high' ? 'danger' : alert.severity === 'medium' ? 'warning' : 'default'}
                          size="sm"
                        >
                          {alert.severity}
                        </Badge>
                        <Badge variant="default" size="sm">
                          {alert.divergence_type}
                        </Badge>
                      </div>
                      <div className="mt-2 text-sm text-text-muted">
                        {alert.wallets_divergent.length} wallets diverged from cluster consensus
                      </div>
                    </div>
                    <div className="text-xs text-text-muted">
                      {new Date(alert.timestamp).toLocaleString()}
                    </div>
                  </div>
                </div>
              ))}
            </div>
          </CardContent>
        </Card>
      )}
    </div>
  )
}
