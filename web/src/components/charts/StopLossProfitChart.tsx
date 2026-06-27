import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card'
import {
  BarChart,
  Bar,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  Legend,
  ResponsiveContainer
} from 'recharts'

interface StrategyData {
  strategy: string
  activations: number
  lossPrevented: number
  averageLoss: number
}

interface ProfitTargetData {
  strategy: string
  hitRate: number
  totalHits: number
  averageGain: number
}

interface StopLossProfitChartProps {
  // Stop Loss Data
  activationRate: number
  totalActivations: number
  lossPreventedSol: number
  averageLossPreventedSol: number
  activationsByStrategy: StrategyData[]

  // Profit Target Data
  hitRate: number
  totalHits: number
  totalTargets: number
  trailingStopActivations: number
  averageRealizedGainSol: number
  targetsByStrategy: ProfitTargetData[]
  recentHits?: Array<{ timestamp: string; token: string; gain: number }>

  className?: string
}

export function StopLossProfitChart({
  activationRate,
  totalActivations,
  lossPreventedSol,
  averageLossPreventedSol,
  activationsByStrategy,
  hitRate,
  totalHits,
  totalTargets,
  trailingStopActivations,
  averageRealizedGainSol,
  targetsByStrategy,
  recentHits = [],
  className = ''
}: StopLossProfitChartProps) {
  // Prepare strategy comparison data
  const strategyComparison = activationsByStrategy.map(sl => {
    const pt = targetsByStrategy.find(pt => pt.strategy === sl.strategy)
    return {
      strategy: sl.strategy,
      stopLossActivations: sl.activations,
      profitHits: pt?.totalHits || 0,
      lossPrevented: sl.lossPrevented,
      gainsRealized: (pt?.averageGain || 0) * (pt?.totalHits || 0)
    }
  })

  // Prepare hit rate comparison
  const hitRateData = targetsByStrategy.map(pt => ({
    strategy: pt.strategy,
    hitRate: (pt.hitRate * 100).toFixed(1),
    targets: pt.totalHits
  }))

  return (
    <Card className={className}>
      <CardHeader>
        <CardTitle className="text-lg">Risk Management Performance</CardTitle>
      </CardHeader>
      <CardContent>
        <div className="space-y-6">
          {/* Overall Performance Metrics */}
          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
            {/* Stop Loss Metrics */}
            <div className="p-4 bg-green-50 rounded-lg">
              <h3 className="text-sm font-medium text-gray-600">Stop Loss Activations</h3>
              <p className="text-2xl font-bold text-green-600">{totalActivations}</p>
              <p className="text-xs text-gray-500">{(activationRate * 100).toFixed(1)}% rate</p>
            </div>

            <div className="p-4 bg-emerald-50 rounded-lg">
              <h3 className="text-sm font-medium text-gray-600">Loss Prevented</h3>
              <p className="text-2xl font-bold text-emerald-600">{lossPreventedSol.toFixed(1)} SOL</p>
              <p className="text-xs text-gray-500">{averageLossPreventedSol.toFixed(2)} SOL avg</p>
            </div>

            {/* Profit Target Metrics */}
            <div className="p-4 bg-blue-50 rounded-lg">
              <h3 className="text-sm font-medium text-gray-600">Profit Targets Hit</h3>
              <p className="text-2xl font-bold text-blue-600">{totalHits}/{totalTargets}</p>
              <p className="text-xs text-gray-500">{(hitRate * 100).toFixed(1)}% hit rate</p>
            </div>

            <div className="p-4 bg-purple-50 rounded-lg">
              <h3 className="text-sm font-medium text-gray-600">Avg Realized Gain</h3>
              <p className="text-2xl font-bold text-purple-600">{averageRealizedGainSol.toFixed(2)} SOL</p>
              <p className="text-xs text-gray-500">{trailingStopActivations} trailing stops</p>
            </div>
          </div>

          {/* Strategy Comparison */}
          <div>
            <h3 className="text-sm font-medium text-gray-600 mb-3">Strategy Performance Comparison</h3>
            <ResponsiveContainer width="100%" height={250}>
              <BarChart data={strategyComparison}>
                <CartesianGrid strokeDasharray="3 3" />
                <XAxis dataKey="strategy" />
                <YAxis />
                <Tooltip
                  formatter={(value: number, name: string) => {
                    if (name === 'lossPrevented') return [`${value.toFixed(1)} SOL`, 'Loss Prevented']
                    if (name === 'gainsRealized') return [`${value.toFixed(1)} SOL`, 'Gains Realized']
                    return [value, name]
                  }}
                />
                <Legend />
                <Bar dataKey="lossPrevented" fill="#10b981" name="Loss Prevented (SOL)" radius={[4, 4, 0, 0]} />
                <Bar dataKey="gainsRealized" fill="#3b82f6" name="Gains Realized (SOL)" radius={[4, 4, 0, 0]} />
              </BarChart>
            </ResponsiveContainer>
          </div>

          {/* Two Column Layout */}
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
            {/* Stop Loss by Strategy */}
            <div>
              <h3 className="text-sm font-medium text-gray-600 mb-3">Stop Loss Activations by Strategy</h3>
              <ResponsiveContainer width="100%" height={200}>
                <BarChart data={activationsByStrategy}>
                  <CartesianGrid strokeDasharray="3 3" />
                  <XAxis dataKey="strategy" />
                  <YAxis />
                  <Tooltip
                    formatter={(value: number, name: string) => {
                      if (name === 'lossPrevented') return [`${value.toFixed(1)} SOL`, 'Loss Prevented']
                      return [value, name]
                    }}
                  />
                  <Legend />
                  <Bar dataKey="activations" fill="#ef4444" name="Activations" radius={[4, 4, 0, 0]} />
                </BarChart>
              </ResponsiveContainer>
            </div>

            {/* Profit Target Hit Rates */}
            <div>
              <h3 className="text-sm font-medium text-gray-600 mb-3">Profit Target Hit Rates</h3>
              <ResponsiveContainer width="100%" height={200}>
                <BarChart data={hitRateData}>
                  <CartesianGrid strokeDasharray="3 3" />
                  <XAxis dataKey="strategy" />
                  <YAxis domain={[0, 100]} label={{ value: 'Hit Rate %', angle: -90, position: 'insideLeft' }} />
                  <Tooltip
                    formatter={(value: number, name: string) => {
                      if (name === 'hitRate') return [`${value}%`, 'Hit Rate']
                      return [value, name]
                    }}
                  />
                  <Legend />
                  <Bar dataKey="hitRate" fill="#10b981" name="Hit Rate %" radius={[4, 4, 0, 0]} />
                </BarChart>
              </ResponsiveContainer>
            </div>
          </div>

          {/* Recent Activity */}
          {recentHits.length > 0 && (
            <div>
              <h3 className="text-sm font-medium text-gray-600 mb-3">Recent Profit Target Hits</h3>
              <div className="space-y-2 max-h-48 overflow-y-auto">
                {recentHits.slice(0, 10).map((hit, index) => (
                  <div
                    key={index}
                    className="flex justify-between items-center p-3 bg-green-50 rounded-lg border border-green-100"
                  >
                    <div>
                      <span className="font-medium text-green-800">{hit.token}</span>
                      <p className="text-xs text-green-600">{new Date(hit.timestamp).toLocaleString()}</p>
                    </div>
                    <span className="text-sm font-semibold text-green-700">
                      +{hit.gain.toFixed(2)} SOL
                    </span>
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* Performance Insights */}
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            {hitRate > 0.6 && (
              <div className="p-3 bg-green-50 border border-green-200 rounded-lg">
                <p className="text-sm text-green-800">
                  <strong>Excellent Performance:</strong> {(hitRate * 100).toFixed(0)}% profit target hit rate indicates effective profit-taking strategy.
                </p>
              </div>
            )}

            {activationRate > 0.1 && lossPreventedSol > 10 && (
              <div className="p-3 bg-blue-50 border border-blue-200 rounded-lg">
                <p className="text-sm text-blue-800">
                  <strong>Risk Protection:</strong> Stop losses prevented {lossPreventedSol.toFixed(0)} SOL in losses, protecting capital from significant drawdowns.
                </p>
              </div>
            )}

            {hitRate < 0.4 && (
              <div className="p-3 bg-yellow-50 border border-yellow-200 rounded-lg">
                <p className="text-sm text-yellow-800">
                  <strong>Optimization Opportunity:</strong> Consider reviewing profit target levels or holding periods to improve hit rate.
                </p>
              </div>
            )}
          </div>
        </div>
      </CardContent>
    </Card>
  )
}