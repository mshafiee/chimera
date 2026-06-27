import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card'
import {
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
  ScatterChart,
  Scatter,
  ZAxis
} from 'recharts'

interface DivergenceAlert {
  timestamp: string
  token_address: string
  token_symbol: string | null
  divergence_score: number
  wallets_divergent: string[]
}

interface ConsensusSignal {
  timestamp: string
  token_address: string
  token_symbol: string | null
  consensus_wallets: number
  total_wallets: number
  quality_score: number
}

interface SignalConsensusChartProps {
  consensusDetectionRate: number
  averageClustering: number
  divergenceAlerts: DivergenceAlert[]
  consensusSignals: ConsensusSignal[]
  className?: string
}

export function SignalConsensusChart({
  consensusDetectionRate,
  averageClustering,
  divergenceAlerts,
  consensusSignals,
  className = ''
}: SignalConsensusChartProps) {
  // Prepare consensus strength data
  const consensusStrengthData = consensusSignals.map(signal => ({
    symbol: signal.token_symbol || 'Unknown',
    consensus_percent: ((signal.consensus_wallets / signal.total_wallets) * 100).toFixed(0),
    quality_score: (signal.quality_score * 100).toFixed(0),
    wallets: signal.consensus_wallets,
    total: signal.total_wallets,
    timestamp: new Date(signal.timestamp).toLocaleDateString()
  }))

  // Prepare divergence data
  const divergenceData = divergenceAlerts.map(alert => ({
    symbol: alert.token_symbol || 'Unknown',
    divergence_score: (alert.divergence_score * 100).toFixed(0),
    wallets_count: alert.wallets_divergent.length,
    timestamp: new Date(alert.timestamp).toLocaleString(),
    high_risk: alert.divergence_score > 0.7
  }))

  return (
    <Card className={className}>
      <CardHeader>
        <CardTitle className="text-lg">Signal Consensus & Clustering</CardTitle>
      </CardHeader>
      <CardContent>
        <div className="space-y-6">
          {/* Key Metrics */}
          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
            {/* Consensus Detection Rate */}
            <div className="p-4 bg-green-50 rounded-lg">
              <h3 className="text-sm font-medium text-gray-600">Consensus Detection</h3>
              <p className="text-2xl font-bold text-green-600">
                {(consensusDetectionRate * 100).toFixed(0)}%
              </p>
              <p className="text-xs text-gray-500">Of all signals</p>
            </div>

            {/* Average Clustering */}
            <div className="p-4 bg-blue-50 rounded-lg">
              <h3 className="text-sm font-medium text-gray-600">Avg Clustering</h3>
              <p className="text-2xl font-bold text-blue-600">
                {(averageClustering * 100).toFixed(0)}%
              </p>
              <p className="text-xs text-gray-500">Wallet agreement</p>
            </div>

            {/* Consensus Signals */}
            <div className="p-4 bg-purple-50 rounded-lg">
              <h3 className="text-sm font-medium text-gray-600">Consensus Signals</h3>
              <p className="text-2xl font-bold text-purple-600">{consensusSignals.length}</p>
              <p className="text-xs text-gray-500">Multi-wallet agreement</p>
            </div>

            {/* Divergence Alerts */}
            <div className="p-4 bg-orange-50 rounded-lg">
              <h3 className="text-sm font-medium text-gray-600">Divergence Alerts</h3>
              <p className="text-2xl font-bold text-orange-600">{divergenceAlerts.length}</p>
              <p className="text-xs text-gray-500">Wallet disagreement</p>
            </div>
          </div>

          {/* Consensus Strength Chart */}
          {consensusStrengthData.length > 0 && (
            <div>
              <h3 className="text-sm font-medium text-gray-600 mb-3">Consensus Signal Strength</h3>
              <ResponsiveContainer width="100%" height={300}>
                <ScatterChart margin={{ top: 20, right: 20, bottom: 20, left: 20 }}>
                  <CartesianGrid strokeDasharray="3 3" />
                  <XAxis
                    type="number"
                    dataKey="consensus_percent"
                    domain={[0, 100]}
                    label={{ value: 'Consensus %', position: 'insideBottom', offset: -5 }}
                  />
                  <YAxis
                    type="number"
                    dataKey="quality_score"
                    domain={[0, 100]}
                    label={{ value: 'Quality %', angle: -90, position: 'insideLeft' }}
                  />
                  <ZAxis type="number" dataKey="wallets" range={[50, 300]} />
                  <Tooltip
                    content={({ payload }) => {
                      if (payload && payload.length > 0) {
                        const data = payload[0].payload
                        return (
                          <div className="bg-white p-2 border rounded shadow">
                            <p className="font-medium">{data.symbol}</p>
                            <p>Consensus: {data.consensus_percent}%</p>
                            <p>Quality: {data.quality_score}%</p>
                            <p>Wallets: {data.wallets}/{data.total}</p>
                            <p className="text-xs text-gray-500">{data.timestamp}</p>
                          </div>
                        )
                      }
                      return null
                    }}
                  />
                  <Scatter data={consensusStrengthData} fill="#10b981" shape="circle" />
                </ScatterChart>
              </ResponsiveContainer>
            </div>
          )}

          {/* Divergence Analysis */}
          {divergenceData.length > 0 && (
            <div>
              <h3 className="text-sm font-medium text-gray-600 mb-3">Recent Divergence Alerts</h3>
              <div className="space-y-2 max-h-64 overflow-y-auto">
                {divergenceData.map((alert, index) => (
                  <div
                    key={index}
                    className={`p-3 rounded-lg border ${
                      alert.high_risk
                        ? 'bg-red-50 border-red-200'
                        : 'bg-yellow-50 border-yellow-200'
                    }`}
                  >
                    <div className="flex justify-between items-start">
                      <div>
                        <span className="font-medium text-gray-800">{alert.symbol}</span>
                        <p className="text-xs text-gray-600">{alert.timestamp}</p>
                      </div>
                      <div className="text-right">
                        <div className="flex items-center gap-2">
                          <span
                            className={`text-sm font-semibold ${
                              alert.high_risk ? 'text-red-700' : 'text-yellow-700'
                            }`}
                          >
                            {(alert.divergence_score)}% divergence
                          </span>
                          <span className="text-xs text-gray-500">
                            {alert.wallets_count} wallets
                          </span>
                        </div>
                      </div>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* High Consensus Signals */}
          {consensusSignals.length > 0 && (
            <div>
              <h3 className="text-sm font-medium text-gray-600 mb-3">High Consensus Signals</h3>
              <div className="space-y-2 max-h-64 overflow-y-auto">
                {consensusSignals.slice(0, 10).map((signal, index) => (
                  <div
                    key={index}
                    className="flex justify-between items-center p-3 bg-green-50 rounded-lg border border-green-100"
                  >
                    <div>
                      <span className="font-medium text-green-800">
                        {signal.token_symbol || 'Unknown'}
                      </span>
                      <p className="text-xs text-green-600">
                        {signal.consensus_wallets}/{signal.total_wallets} wallets
                      </p>
                    </div>
                    <div className="text-right">
                      <span className="text-sm font-semibold text-green-700">
                        {((signal.consensus_wallets / signal.total_wallets) * 100).toFixed(0)}%
                      </span>
                      <p className="text-xs text-gray-500">
                        Quality: {(signal.quality_score * 100).toFixed(0)}%
                      </p>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* Clustering Insights */}
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            {consensusDetectionRate > 0.5 && (
              <div className="p-3 bg-green-50 border border-green-200 rounded-lg">
                <p className="text-sm text-green-800">
                  <strong>Strong Consensus:</strong> {(consensusDetectionRate * 100).toFixed(0)}% of
                  signals show multi-wallet agreement, indicating high confidence trading opportunities.
                </p>
              </div>
            )}

            {divergenceAlerts.length > 5 && (
              <div className="p-3 bg-yellow-50 border border-yellow-200 rounded-lg">
                <p className="text-sm text-yellow-800">
                  <strong>Elevated Divergence:</strong> {divergenceAlerts.length} divergence alerts detected.
                  Review conflicting signals before execution.
                </p>
              </div>
            )}

            {averageClustering < 0.5 && consensusDetectionRate < 0.3 && (
              <div className="p-3 bg-blue-50 border border-blue-200 rounded-lg">
                <p className="text-sm text-blue-800">
                  <strong>Low Agreement:</strong> Wallet clustering and consensus below optimal. Consider
                  expanding tracked wallet pool or adjusting consensus thresholds.
                </p>
              </div>
            )}

            {consensusSignals.some(s => (s.consensus_wallets / s.total_wallets) > 0.8) && (
              <div className="p-3 bg-purple-50 border border-purple-200 rounded-lg">
                <p className="text-sm text-purple-800">
                  <strong>High Confidence Opportunities:</strong> Several signals show 80%+ wallet agreement.
                  These represent the highest quality trading opportunities.
                </p>
              </div>
            )}
          </div>
        </div>
      </CardContent>
    </Card>
  )
}