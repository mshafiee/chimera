import { Card, CardHeader, CardTitle, CardContent } from '../components/ui/Card'
import { Badge } from '../components/ui/Badge'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../components/ui/Table'
import { useSignalQuality, useSignalSources, useSignalConsensus } from '../api'
import { SignalQualityChart } from '../components/signals/SignalQualityChart'
import { SignalSourcesTable } from '../components/signals/SignalSourcesTable'
import { MetricCard } from '../components/ui/MetricCard'
import { TimeRangePicker, TimeRange } from '../components/ui/TimeRangePicker'
import { useState } from 'react'

export function Signals() {
  const [timeRange, setTimeRange] = useState<TimeRange>('24h')

  const { data: signalQuality, isLoading: qualityLoading } = useSignalQuality(timeRange)
  const { data: signalSources, isLoading: sourcesLoading } = useSignalSources()
  const { data: signalConsensus, isLoading: consensusLoading } = useSignalConsensus()

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold">Signal Intelligence</h1>
          <p className="text-text-muted text-sm">Signal quality and consensus analysis</p>
        </div>
        <TimeRangePicker value={timeRange} onChange={setTimeRange} />
      </div>

      {/* Signal Quality Metrics */}
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
        <MetricCard
          label="Current Quality Score"
          value={signalQuality?.current_quality_score?.toFixed(2) || '0.00'}
          loading={qualityLoading}
          positive={(signalQuality?.current_quality_score || 0) >= 0.7}
          unit="★"
        />
        <MetricCard
          label="Total Signals"
          value={signalQuality?.total_signals || 0}
          loading={qualityLoading}
        />
        <MetricCard
          label="Accepted"
          value={signalQuality?.accepted_signals || 0}
          loading={qualityLoading}
          positive
          icon="✓"
        />
        <MetricCard
          label="Rejected"
          value={signalQuality?.rejected_signals || 0}
          loading={qualityLoading}
          positive={false}
          icon="✕"
        />
      </div>

      {/* Signal Quality Distribution */}
      <Card>
        <CardHeader>
          <CardTitle>Signal Quality Distribution</CardTitle>
        </CardHeader>
        <CardContent>
          {qualityLoading ? (
            <div className="text-center text-text-muted py-8">Loading signal quality...</div>
          ) : signalQuality ? (
            <SignalQualityChart data={signalQuality} />
          ) : (
            <div className="text-center text-text-muted py-8">No signal quality data available</div>
          )}
        </CardContent>
      </Card>

      {/* Signal Sources */}
      <Card>
        <CardHeader>
          <CardTitle>Signal Sources</CardTitle>
        </CardHeader>
        <CardContent>
          {sourcesLoading ? (
            <div className="text-center text-text-muted py-8">Loading signal sources...</div>
          ) : signalSources ? (
            <SignalSourcesTable sources={signalSources.sources} />
          ) : (
            <div className="text-center text-text-muted py-8">No signal sources available</div>
          )}
        </CardContent>
      </Card>

      {/* Signal Consensus */}
      <Card>
        <CardHeader>
          <div className="flex items-center justify-between">
            <CardTitle>Signal Consensus</CardTitle>
            <Badge variant={(signalConsensus?.consensus_detection_rate ?? 0) > 0.7 ? 'success' : 'warning'}>
              {signalConsensus?.consensus_detection_rate !== null && signalConsensus?.consensus_detection_rate !== undefined
                ? `${(signalConsensus.consensus_detection_rate * 100).toFixed(1)}%`
                : 'N/A'}
            </Badge>
          </div>
        </CardHeader>
        <CardContent>
          {consensusLoading ? (
            <div className="text-center text-text-muted py-8">Loading consensus data...</div>
          ) : signalConsensus ? (
            <div className="space-y-6">
              {/* Consensus Metrics */}
              <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
                <div className="bg-surface-light rounded-lg p-4">
                  <div className="text-xs text-text-muted mb-1">Consensus Rate</div>
                  <div className="text-xl font-semibold font-mono-numbers">
                    {(signalConsensus.consensus_detection_rate * 100).toFixed(1)}%
                  </div>
                </div>
                <div className="bg-surface-light rounded-lg p-4">
                  <div className="text-xs text-text-muted mb-1">Avg Clustering</div>
                  <div className="text-xl font-semibold font-mono-numbers">
                    {signalConsensus.average_clustering.toFixed(2)}
                  </div>
                </div>
                <div className="bg-surface-light rounded-lg p-4">
                  <div className="text-xs text-text-muted mb-1">Divergence Alerts</div>
                  <div className="text-xl font-semibold font-mono-numbers">
                    {signalConsensus.divergence_alerts.length}
                  </div>
                </div>
              </div>

              {/* Recent Consensus Signals */}
              {signalConsensus.consensus_signals.length > 0 && (
                <div>
                  <h3 className="text-sm font-medium mb-3">Recent Consensus Signals</h3>
                  <Table>
                    <TableHeader>
                      <TableRow hoverable={false}>
                        <TableHead>Token</TableHead>
                        <TableHead>Wallets</TableHead>
                        <TableHead>Quality</TableHead>
                        <TableHead>Executed</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {signalConsensus.consensus_signals.slice(0, 10).map((signal, index) => (
                        <TableRow key={index}>
                          <TableCell>
                            <div className="font-semibold">
                              ${signal.token_symbol || 'Unknown'}
                            </div>
                            <div className="text-xs text-text-muted">
                              {signal.token_address.slice(0, 8)}...
                            </div>
                          </TableCell>
                          <TableCell>
                            <div className="text-sm">
                              {signal.consensus_wallets} / {signal.total_wallets}
                            </div>
                          </TableCell>
                          <TableCell mono className="text-sm">
                            {signal.quality_score.toFixed(2)}
                          </TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                </div>
              )}

              {/* Divergence Alerts */}
              {signalConsensus.divergence_alerts.length > 0 && (
                <div>
                  <h3 className="text-sm font-medium mb-3 text-loss">Divergence Alerts</h3>
                  <div className="space-y-2">
                    {signalConsensus.divergence_alerts.map((alert, index) => (
                      <div key={index} className="bg-loss/10 border border-loss/30 rounded-lg p-3">
                        <div className="flex items-center justify-between">
                          <div className="flex items-center gap-3">
                            <div className="font-semibold text-sm">
                              ${alert.token_symbol || 'Unknown'}
                            </div>
                          </div>
                        </div>
                        <div className="mt-2 text-xs text-text-muted">
                          {alert.wallets_divergent.length} wallets diverged from cluster
                        </div>
                      </div>
                    ))}
                  </div>
                </div>
              )}
            </div>
          ) : (
            <div className="text-center text-text-muted py-8">No consensus data available</div>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
