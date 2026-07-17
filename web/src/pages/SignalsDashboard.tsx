import { useState } from 'react'
import { TimeRangePicker } from '@/components/ui/TimeRangePicker'
import { SignalQualityChart } from '@/components/charts/SignalQualityChart'
import { SignalConsensusChart } from '@/components/charts/SignalConsensusChart'
import { RealTimeAlerts, ConnectionStatus } from '@/components/dashboard'
import { useDashboardWebSocket } from '@/hooks/useDashboardWebSocket'
import { useWebSocket } from '@/hooks/useWebSocket'
import { useAuthStore } from '../stores/authStore'
import {
  useSignalQuality,
  useSignalSources,
  useSignalConsensus,
  useSignalAggregation,
  useSignalClustering
} from '@/api/signals'
import type { TimeRange } from '@/components/ui/TimeRangePicker'

export function SignalsDashboard() {
  const [timeRange, setTimeRange] = useState<TimeRange>('24h')

  // WebSocket integration
  const userToken = useAuthStore(state => state.user?.token) ?? ''
  const { isConnected, isConnecting, connectionError } = useWebSocket({ apiKey: userToken })
  const { refreshSignalData } = useDashboardWebSocket({
    onSignalUpdate: (data) => {
      console.log('Signal update received:', data)
    },
    onConsensusAlert: (data) => {
      console.log('Consensus alert received:', data)
    },
    onQualityChange: (data) => {
      console.log('Quality change received:', data)
    },
  })

  // Fetch data from API
  const { data: signalQuality, isLoading: qualityLoading } = useSignalQuality(timeRange)
  const { data: signalSources, isLoading: sourcesLoading } = useSignalSources()
  const { data: signalConsensus, isLoading: consensusLoading } = useSignalConsensus()
  const { data: signalAggregation, isLoading: aggregationLoading } = useSignalAggregation(timeRange)
  const { data: signalClustering, isLoading: clusteringLoading } = useSignalClustering()

  const isLoading = qualityLoading || sourcesLoading || consensusLoading || aggregationLoading || clusteringLoading

  if (isLoading) {
    return (
      <div className="flex items-center justify-center min-h-screen">
        <div className="text-center">
          <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-shield mx-auto mb-4"></div>
          <p className="text-text-muted">Loading signal analysis...</p>
        </div>
      </div>
    )
  }

  return (
    <div className="container mx-auto px-4 py-8">
      {/* Real-time components */}
      <RealTimeAlerts maxAlerts={3} />

      {/* Header */}
      <div className="mb-8 flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-bold text-gray-900 mb-2">Signal Analysis Dashboard</h1>
          <p className="text-gray-600">Real-time signal quality, consensus, and wallet clustering analysis</p>
        </div>
        <ConnectionStatus
          isConnected={isConnected}
          isConnecting={isConnecting}
          connectionError={connectionError}
        />
      </div>

      {/* Time Range Selector */}
      <div className="mb-6 flex items-center gap-4">
        <TimeRangePicker value={timeRange} onChange={setTimeRange} />
        <button
          onClick={refreshSignalData}
          className="px-4 py-2 text-sm font-medium text-white bg-shield rounded-lg hover:bg-shield/90 transition-colors"
        >
          Refresh Data
        </button>
      </div>

      {/* Dashboard Grid */}
      <div className="space-y-6">
        {/* First Row: Signal Quality & Consensus */}
        <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
          {signalQuality && (
            <SignalQualityChart
              currentQualityScore={signalQuality.current_quality_score}
              qualityDistribution={signalQuality.quality_distribution}
              rejectionRate={signalQuality.rejection_rate}
              totalSignals={signalQuality.total_signals}
              acceptedSignals={signalQuality.accepted_signals}
              rejectedSignals={signalQuality.rejected_signals}
              averageQualityTrend={signalQuality.average_quality_trend}
            />
          )}

          {signalConsensus && (
            <SignalConsensusChart
              consensusDetectionRate={signalConsensus.consensus_detection_rate}
              averageClustering={signalConsensus.average_clustering}
              divergenceAlerts={signalConsensus.divergence_alerts}
              consensusSignals={signalConsensus.consensus_signals}
            />
          )}
        </div>

        {/* Second Row: Signal Sources */}
        {signalSources && signalSources.sources.length > 0 && (
          <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
            {/* Top Sources */}
            <div className="p-6 bg-white rounded-lg shadow">
              <h3 className="text-lg font-semibold text-gray-900 mb-4">Top Signal Sources</h3>
              <div className="space-y-3">
                {signalSources.sources.slice(0, 10).map((source, index) => (
                  <div
                    key={index}
                    className="flex justify-between items-center p-3 bg-surface-light rounded-lg"
                  >
                    <div>
                      <div className="flex items-center gap-2">
                        <span className="font-medium text-gray-800">
                          {source.source.slice(0, 8)}...{source.source.slice(-4)}
                        </span>
                        <span className="text-xs px-2 py-1 bg-shield/10 text-shield rounded">
                          #{index + 1}
                        </span>
                      </div>
                      <p className="text-xs text-gray-500 mt-1">
                        Last: {new Date(source.last_signal_at).toLocaleDateString()}
                      </p>
                    </div>
                    <div className="text-right">
                      <p className="text-sm font-semibold text-gray-900">{source.signal_count}</p>
                      <p className="text-xs text-gray-500">signals</p>
                    </div>
                  </div>
                ))}
              </div>
            </div>

            {/* Source Performance */}
            <div className="p-6 bg-white rounded-lg shadow">
              <h3 className="text-lg font-semibold text-gray-900 mb-4">Source Performance</h3>
              <div className="space-y-3">
                {signalSources.sources
                  .sort((a, b) => b.average_quality - a.average_quality)
                  .slice(0, 8)
                  .map((source, index) => (
                    <div key={index} className="space-y-2">
                      <div className="flex justify-between items-center">
                        <span className="text-sm text-gray-600">
                          {source.source.slice(0, 8)}...{source.source.slice(-4)}
                        </span>
                        <span className="text-sm font-medium text-shield">
                          {(source.average_quality * 100).toFixed(0)}%
                        </span>
                      </div>
                      <div className="w-full bg-gray-200 rounded-full h-2">
                        <div
                          className="bg-shield h-2 rounded-full"
                          style={{ width: `${source.average_quality * 100}%` }}
                        ></div>
                      </div>
                    </div>
                  ))}
              </div>
            </div>

            {/* Acceptance Rates */}
            <div className="p-6 bg-white rounded-lg shadow">
              <h3 className="text-lg font-semibold text-gray-900 mb-4">Acceptance Rates</h3>
              <div className="space-y-3">
                {signalSources.sources
                  .sort((a, b) => b.acceptance_rate - a.acceptance_rate)
                  .slice(0, 8)
                  .map((source, index) => (
                    <div key={index} className="space-y-2">
                      <div className="flex justify-between items-center">
                        <span className="text-sm text-gray-600">
                          {source.source.slice(0, 8)}...{source.source.slice(-4)}
                        </span>
                        <span className="text-sm font-medium text-green-600">
                          {(source.acceptance_rate * 100).toFixed(0)}%
                        </span>
                      </div>
                      <div className="w-full bg-gray-200 rounded-full h-2">
                        <div
                          className="bg-green-500 h-2 rounded-full"
                          style={{ width: `${source.acceptance_rate * 100}%` }}
                        ></div>
                      </div>
                    </div>
                  ))}
              </div>
            </div>
          </div>
        )}

        {/* Third Row: Signal Aggregation */}
        {signalAggregation && (
          <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
            {/* Aggregation Metrics */}
            <div className="p-6 bg-gradient-to-br from-blue-50 to-blue-100 rounded-lg shadow">
              <h3 className="text-sm font-medium text-gray-600 mb-2">Aggregated Windows</h3>
              <p className="text-3xl font-bold text-blue-600">
                {signalAggregation.total_aggregated_windows}
              </p>
              <p className="text-xs text-gray-500 mt-1">5-minute time windows</p>
            </div>

            <div className="p-6 bg-gradient-to-br from-purple-50 to-purple-100 rounded-lg shadow">
              <h3 className="text-sm font-medium text-gray-600 mb-2">Avg Signals/Window</h3>
              <p className="text-3xl font-bold text-purple-600">
                {signalAggregation.average_signals_per_window.toFixed(1)}
              </p>
              <p className="text-xs text-gray-500 mt-1">Signal density</p>
            </div>

            <div className="p-6 bg-gradient-to-br from-green-50 to-green-100 rounded-lg shadow">
              <h3 className="text-sm font-medium text-gray-600 mb-2">Top Tokens</h3>
              <p className="text-3xl font-bold text-green-600">
                {signalAggregation.top_aggregated_tokens.length}
              </p>
              <p className="text-xs text-gray-500 mt-1">Most aggregated tokens</p>
            </div>
          </div>
        )}

        {/* Aggregation Trend */}
        {signalAggregation && signalAggregation.aggregation_trend.length > 0 && (
          <div className="p-6 bg-white rounded-lg shadow">
            <h3 className="text-lg font-semibold text-gray-900 mb-4">Signal Aggregation Trend</h3>
            <div className="space-y-2 max-h-64 overflow-y-auto">
              {signalAggregation.aggregation_trend.slice(0, 20).map((point, index) => (
                <div key={index} className="flex items-center justify-between p-2 bg-surface-light rounded">
                  <span className="text-sm text-gray-600">
                    {new Date(point.timestamp).toLocaleString()}
                  </span>
                  <div className="flex items-center gap-4">
                    <span className="text-sm text-gray-800">{point.signal_count} signals</span>
                    <span className="text-sm text-blue-600">{point.window_count} windows</span>
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}

        {/* Top Aggregated Tokens */}
        {signalAggregation && signalAggregation.top_aggregated_tokens.length > 0 && (
          <div className="p-6 bg-white rounded-lg shadow">
            <h3 className="text-lg font-semibold text-gray-900 mb-4">Top Aggregated Tokens</h3>
            <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
              {signalAggregation.top_aggregated_tokens.map((token, index) => (
                <div key={index} className="p-4 bg-surface-light rounded-lg border-l-4 border-shield">
                  <div className="flex items-start justify-between">
                    <div>
                      <span className="font-medium text-gray-900">
                        {token.token_symbol || 'Unknown'}
                      </span>
                      <p className="text-xs text-gray-500 mt-1">
                        {token.token_address.slice(0, 8)}...{token.token_address.slice(-4)}
                      </p>
                    </div>
                    <span className="text-xs px-2 py-1 bg-shield/10 text-shield rounded font-medium">
                      #{index + 1}
                    </span>
                  </div>
                  <div className="mt-3 space-y-1">
                    <div className="flex justify-between text-sm">
                      <span className="text-gray-600">Aggregated signals:</span>
                      <span className="font-medium text-gray-900">{token.aggregated_signal_count}</span>
                    </div>
                    <div className="flex justify-between text-sm">
                      <span className="text-gray-600">Unique wallets:</span>
                      <span className="font-medium text-gray-900">{token.unique_wallets}</span>
                    </div>
                    <div className="flex justify-between text-sm">
                      <span className="text-gray-600">Avg quality:</span>
                      <span className="font-medium text-shield">
                        {(token.average_quality_score * 100).toFixed(0)}%
                      </span>
                    </div>
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}

        {/* Fourth Row: Wallet Clustering */}
        {signalClustering && (
          <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
            {/* Clustering Metrics */}
            <div className="p-6 bg-gradient-to-br from-shield/10 to-shield/5 rounded-lg shadow border border-shield/20">
              <h3 className="text-sm font-medium text-gray-600 mb-2">Total Clusters</h3>
              <p className="text-3xl font-bold text-shield">{signalClustering.total_clusters}</p>
              <p className="text-xs text-gray-500 mt-1">Wallet groups identified</p>
            </div>

            <div className="p-6 bg-gradient-to-br from-blue-50 to-blue-100 rounded-lg shadow">
              <h3 className="text-sm font-medium text-gray-600 mb-2">Avg Cluster Size</h3>
              <p className="text-3xl font-bold text-blue-600">
                {signalClustering.average_cluster_size.toFixed(1)}
              </p>
              <p className="text-xs text-gray-500 mt-1">Wallets per cluster</p>
            </div>

            <div className="p-6 bg-gradient-to-br from-purple-50 to-purple-100 rounded-lg shadow">
              <h3 className="text-sm font-medium text-gray-600 mb-2">Clustering Coefficient</h3>
              <p className="text-3xl font-bold text-purple-600">
                {(signalClustering.clustering_coefficient * 100).toFixed(0)}%
              </p>
              <p className="text-xs text-gray-500 mt-1">Network connectivity</p>
            </div>
          </div>
        )}

        {/* Cluster Details */}
        {signalClustering && signalClustering.clusters.length > 0 && (
          <div className="p-6 bg-white rounded-lg shadow">
            <h3 className="text-lg font-semibold text-gray-900 mb-4">Wallet Clusters</h3>
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              {signalClustering.clusters.map((cluster) => (
                <div key={cluster.cluster_id} className="p-4 bg-surface-light rounded-lg">
                  <div className="flex items-start justify-between mb-3">
                    <div>
                      <h4 className="font-medium text-gray-900">Cluster #{cluster.cluster_id}</h4>
                      <p className="text-sm text-gray-600">{cluster.size} wallets</p>
                    </div>
                    <div className="flex gap-2">
                      <span className="text-xs px-2 py-1 bg-shield/10 text-shield rounded">
                        Quality: {(cluster.average_quality * 100).toFixed(0)}%
                      </span>
                      <span className="text-xs px-2 py-1 bg-blue-10 text-blue-600 rounded">
                        Consensus: {(cluster.consensus_rate * 100).toFixed(0)}%
                      </span>
                    </div>
                  </div>

                  <div className="space-y-2">
                    <div>
                      <p className="text-xs text-gray-500 mb-1">Wallet Addresses:</p>
                      <div className="flex flex-wrap gap-1">
                        {cluster.wallet_addresses.slice(0, 6).map((wallet, index) => (
                          <span
                            key={index}
                            className="text-xs px-2 py-1 bg-white rounded border"
                          >
                            {wallet.slice(0, 6)}...
                          </span>
                        ))}
                        {cluster.wallet_addresses.length > 6 && (
                          <span className="text-xs px-2 py-1 text-gray-500">
                            +{cluster.wallet_addresses.length - 6} more
                          </span>
                        )}
                      </div>
                    </div>

                    <div>
                      <p className="text-xs text-gray-500 mb-1">Common Tokens:</p>
                      <div className="flex flex-wrap gap-1">
                        {cluster.common_tokens.map((token, index) => (
                          <span
                            key={index}
                            className="text-xs px-2 py-1 bg-shield/10 text-shield rounded"
                          >
                            {token.slice(0, 8)}...
                          </span>
                        ))}
                      </div>
                    </div>
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>

      {/* Footer */}
      <div className="mt-8 text-center text-sm text-gray-500">
        <p>Last updated: {new Date().toLocaleString()}</p>
        <p className="mt-1">
          Connection Status: {isConnected ? '🟢 Live' : isConnecting ? '🟡 Connecting...' : '🔴 Disconnected'}
        </p>
        <p className="mt-1">Data refreshes every 15-30 seconds depending on the metric</p>
      </div>
    </div>
  )
}