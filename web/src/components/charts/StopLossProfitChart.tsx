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

// Theme tokens
const COLOR_PROFIT = '#00FF88'
const COLOR_LOSS = '#FF4444'
const COLOR_SHIELD = '#00D4FF'

const AXIS_TICK = { fill: '#888888', fontSize: 12 }
const AXIS_STROKE = '#3A3A3A'
const GRID_STROKE = '#3A3A3A'

const LEGEND_WRAPPER = { fontSize: '12px', color: '#888888' }

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

  const StrategyComparisonTooltip = ({ active, payload, label }: any) => {
    if (active && payload && payload.length) {
      return (
        <div className="bg-surface border border-border rounded-lg p-3 shadow-lg">
          <p className="text-sm font-medium mb-1">{label}</p>
          {payload.map((entry: any, index: number) => (
            <p key={index} className="text-xs text-text-muted font-mono-numbers">
              <span>{entry.name}: </span>
              <span style={{ color: entry.color || entry.fill }}>{(entry.value as number).toFixed(1)} SOL</span>
            </p>
          ))}
        </div>
      )
    }
    return null
  }

  const ActivationsTooltip = ({ active, payload, label }: any) => {
    if (active && payload && payload.length) {
      return (
        <div className="bg-surface border border-border rounded-lg p-3 shadow-lg">
          <p className="text-sm font-medium mb-1">{label}</p>
          <p className="text-xs text-text-muted font-mono-numbers">
            <span>Activations: </span>
            <span style={{ color: payload[0].color || payload[0].fill }}>{payload[0].value}</span>
          </p>
        </div>
      )
    }
    return null
  }

  const HitRateTooltip = ({ active, payload, label }: any) => {
    if (active && payload && payload.length) {
      return (
        <div className="bg-surface border border-border rounded-lg p-3 shadow-lg">
          <p className="text-sm font-medium mb-1">{label}</p>
          <p className="text-xs text-text-muted font-mono-numbers">
            <span>Hit Rate: </span>
            <span style={{ color: payload[0].color || payload[0].fill }}>{payload[0].value}%</span>
          </p>
        </div>
      )
    }
    return null
  }

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
            <div className="p-4 bg-profit/10 rounded-lg">
              <h3 className="text-sm font-medium text-text-muted">Stop Loss Activations</h3>
              <p className="text-2xl font-bold font-mono-numbers text-profit">{totalActivations}</p>
              <p className="text-xs text-text-muted">{(activationRate * 100).toFixed(1)}% rate</p>
            </div>

            <div className="p-4 bg-profit/10 rounded-lg">
              <h3 className="text-sm font-medium text-text-muted">Loss Prevented</h3>
              <p className="text-2xl font-bold font-mono-numbers text-profit">{lossPreventedSol.toFixed(1)} SOL</p>
              <p className="text-xs text-text-muted">{averageLossPreventedSol.toFixed(2)} SOL avg</p>
            </div>

            {/* Profit Target Metrics */}
            <div className="p-4 bg-shield/10 rounded-lg">
              <h3 className="text-sm font-medium text-text-muted">Profit Targets Hit</h3>
              <p className="text-2xl font-bold font-mono-numbers text-shield">{totalHits}/{totalTargets}</p>
              <p className="text-xs text-text-muted">{(hitRate * 100).toFixed(1)}% hit rate</p>
            </div>

            <div className="p-4 bg-shield/10 rounded-lg">
              <h3 className="text-sm font-medium text-text-muted">Avg Realized Gain</h3>
              <p className="text-2xl font-bold font-mono-numbers text-shield">{averageRealizedGainSol.toFixed(2)} SOL</p>
              <p className="text-xs text-text-muted">{trailingStopActivations} trailing stops</p>
            </div>
          </div>

          {/* Strategy Comparison */}
          <div>
            <h3 className="text-sm font-medium text-text-muted mb-3">Strategy Performance Comparison</h3>
            <ResponsiveContainer width="100%" height={250}>
              <BarChart data={strategyComparison}>
                <CartesianGrid strokeDasharray="3 3" stroke={GRID_STROKE} opacity={0.3} vertical={false} />
                <XAxis dataKey="strategy" tick={AXIS_TICK} stroke={AXIS_STROKE} tickLine={false} />
                <YAxis tick={AXIS_TICK} stroke={AXIS_STROKE} tickLine={false} />
                <Tooltip content={<StrategyComparisonTooltip />} cursor={{ fill: '#2E2E2E', opacity: 0.5 }} />
                <Legend wrapperStyle={LEGEND_WRAPPER} />
                <Bar dataKey="lossPrevented" fill={COLOR_PROFIT} name="Loss Prevented (SOL)" radius={[4, 4, 0, 0]} />
                <Bar dataKey="gainsRealized" fill={COLOR_SHIELD} name="Gains Realized (SOL)" radius={[4, 4, 0, 0]} />
              </BarChart>
            </ResponsiveContainer>
          </div>

          {/* Two Column Layout */}
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
            {/* Stop Loss by Strategy */}
            <div>
              <h3 className="text-sm font-medium text-text-muted mb-3">Stop Loss Activations by Strategy</h3>
              <ResponsiveContainer width="100%" height={200}>
                <BarChart data={activationsByStrategy}>
                  <CartesianGrid strokeDasharray="3 3" stroke={GRID_STROKE} opacity={0.3} vertical={false} />
                  <XAxis dataKey="strategy" tick={AXIS_TICK} stroke={AXIS_STROKE} tickLine={false} />
                  <YAxis tick={AXIS_TICK} stroke={AXIS_STROKE} tickLine={false} />
                  <Tooltip content={<ActivationsTooltip />} cursor={{ fill: '#2E2E2E', opacity: 0.5 }} />
                  <Legend wrapperStyle={LEGEND_WRAPPER} />
                  <Bar dataKey="activations" fill={COLOR_LOSS} name="Activations" radius={[4, 4, 0, 0]} />
                </BarChart>
              </ResponsiveContainer>
            </div>

            {/* Profit Target Hit Rates */}
            <div>
              <h3 className="text-sm font-medium text-text-muted mb-3">Profit Target Hit Rates</h3>
              <ResponsiveContainer width="100%" height={200}>
                <BarChart data={hitRateData}>
                  <CartesianGrid strokeDasharray="3 3" stroke={GRID_STROKE} opacity={0.3} vertical={false} />
                  <XAxis dataKey="strategy" tick={AXIS_TICK} stroke={AXIS_STROKE} tickLine={false} />
                  <YAxis
                    domain={[0, 100]}
                    tick={AXIS_TICK}
                    stroke={AXIS_STROKE}
                    tickLine={false}
                    label={{ value: 'Hit Rate %', angle: -90, position: 'insideLeft', fill: '#888888' }}
                  />
                  <Tooltip content={<HitRateTooltip />} cursor={{ fill: '#2E2E2E', opacity: 0.5 }} />
                  <Legend wrapperStyle={LEGEND_WRAPPER} />
                  <Bar dataKey="hitRate" fill={COLOR_PROFIT} name="Hit Rate %" radius={[4, 4, 0, 0]} />
                </BarChart>
              </ResponsiveContainer>
            </div>
          </div>

          {/* Recent Activity */}
          {recentHits.length > 0 && (
            <div>
              <h3 className="text-sm font-medium text-text-muted mb-3">Recent Profit Target Hits</h3>
              <div className="space-y-2 max-h-48 overflow-y-auto">
                {recentHits.slice(0, 10).map((hit, index) => (
                  <div
                    key={index}
                    className="flex justify-between items-center p-3 bg-profit/5 rounded-lg border border-profit/20"
                  >
                    <div>
                      <span className="font-medium text-profit">{hit.token}</span>
                      <p className="text-xs text-text-muted">{new Date(hit.timestamp).toLocaleString()}</p>
                    </div>
                    <span className="text-sm font-semibold font-mono-numbers text-profit">
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
              <div className="p-3 bg-profit/10 border border-profit/30 rounded-lg">
                <p className="text-sm text-profit">
                  <strong>Excellent Performance:</strong> {(hitRate * 100).toFixed(0)}% profit target hit rate indicates effective profit-taking strategy.
                </p>
              </div>
            )}

            {activationRate > 0.1 && lossPreventedSol > 10 && (
              <div className="p-3 bg-shield/10 border border-shield/30 rounded-lg">
                <p className="text-sm text-shield">
                  <strong>Risk Protection:</strong> Stop losses prevented {lossPreventedSol.toFixed(0)} SOL in losses, protecting capital from significant drawdowns.
                </p>
              </div>
            )}

            {hitRate < 0.4 && (
              <div className="p-3 bg-spear/10 border border-spear/30 rounded-lg">
                <p className="text-sm text-spear">
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
