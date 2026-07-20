import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card'
import {
  AreaChart,
  Area,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
  ReferenceLine
} from 'recharts'

interface DrawdownDataPoint {
  timestamp: string
  drawdown: number
  portfolio_value: number
}

interface DrawdownChartProps {
  currentDrawdownPercent: number
  maxDrawdownPercent: number
  drawdownDurationDays: number
  recoveryPercent: number
  historicalData?: DrawdownDataPoint[]
  className?: string
}

// Theme tokens
const COLOR_PROFIT = '#00FF88'
const COLOR_SPEAR = '#FF8800'
const COLOR_LOSS = '#FF4444'

const AXIS_TICK = { fill: '#888888', fontSize: 12 }
const AXIS_STROKE = '#3A3A3A'
const GRID_STROKE = '#3A3A3A'

export function DrawdownChart({
  currentDrawdownPercent,
  maxDrawdownPercent,
  drawdownDurationDays,
  recoveryPercent,
  historicalData = [],
  className = ''
}: DrawdownChartProps) {
  // Generate sample historical data if not provided
  const chartData = historicalData.length > 0 ? historicalData : generateSampleData()

  // Get drawdown status
  const getDrawdownStatus = (current: number, max: number) => {
    const ratio = current / max
    if (ratio < 0.3) return { status: 'Minor', color: COLOR_PROFIT }
    if (ratio < 0.6) return { status: 'Moderate', color: COLOR_SPEAR }
    return { status: 'Significant', color: COLOR_LOSS }
  }

  const drawdownStatus = getDrawdownStatus(currentDrawdownPercent, maxDrawdownPercent)

  // Generate sample data for demonstration
  function generateSampleData(): DrawdownDataPoint[] {
    const data: DrawdownDataPoint[] = []
    const now = new Date()
    const days = Math.max(drawdownDurationDays + 10, 30) // At least 30 days of data

    for (let i = days; i >= 0; i--) {
      const date = new Date(now)
      date.setDate(date.getDate() - i)

      // Generate realistic drawdown pattern
      let drawdown = 0
      if (i < drawdownDurationDays) {
        // Drawdown period
        const progress = i / drawdownDurationDays
        drawdown = maxDrawdownPercent * Math.sin(progress * Math.PI / 2) * (0.8 + Math.random() * 0.2)
      } else if (i < drawdownDurationDays + 10) {
        // Recovery period
        const recoveryProgress = (i - drawdownDurationDays) / 10
        drawdown = maxDrawdownPercent * (1 - recoveryProgress) * (0.7 + Math.random() * 0.2)
      }

      data.push({
        timestamp: date.toISOString().split('T')[0],
        drawdown: parseFloat(drawdown.toFixed(2)),
        portfolio_value: 100000 * (1 - drawdown / 100)
      })
    }

    return data
  }

  const CustomTooltip = ({ active, payload, label }: any) => {
    if (active && payload && payload.length) {
      return (
        <div className="bg-surface border border-border rounded-lg p-3 shadow-lg">
          <p className="text-xs text-text-muted mb-1">{label ? new Date(label).toLocaleDateString() : ''}</p>
          <p className="text-sm">
            <span className="text-text-muted">Drawdown: </span>
            <span className="font-medium font-mono-numbers" style={{ color: COLOR_LOSS }}>
              {payload[0].value.toFixed(1)}%
            </span>
          </p>
        </div>
      )
    }
    return null
  }

  return (
    <Card className={className}>
      <CardHeader>
        <CardTitle className="text-lg">Drawdown Analysis</CardTitle>
      </CardHeader>
      <CardContent>
        {/* Key Metrics */}
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4 mb-6">
          {/* Current Drawdown */}
          <div className="p-4 bg-loss/10 rounded-lg">
            <h3 className="text-sm font-medium text-text-muted">Current Drawdown</h3>
            <p className="text-2xl font-bold font-mono-numbers text-loss">
              {currentDrawdownPercent.toFixed(1)}%
            </p>
            <p className="text-xs text-text-muted">From peak</p>
          </div>

          {/* Maximum Drawdown */}
          <div className="p-4 bg-spear/10 rounded-lg">
            <h3 className="text-sm font-medium text-text-muted">Max Drawdown</h3>
            <p className="text-2xl font-bold font-mono-numbers text-spear">
              {maxDrawdownPercent.toFixed(1)}%
            </p>
            <p className="text-xs text-text-muted">Historical worst</p>
          </div>

          {/* Duration */}
          <div className="p-4 bg-shield/10 rounded-lg">
            <h3 className="text-sm font-medium text-text-muted">Drawdown Duration</h3>
            <p className="text-2xl font-bold font-mono-numbers text-shield">
              {drawdownDurationDays}
            </p>
            <p className="text-xs text-text-muted">Days since peak</p>
          </div>

          {/* Recovery Progress */}
          <div className="p-4 bg-profit/10 rounded-lg">
            <h3 className="text-sm font-medium text-text-muted">Recovery Progress</h3>
            <p className="text-2xl font-bold font-mono-numbers text-profit">
              {recoveryPercent.toFixed(0)}%
            </p>
            <p className="text-xs text-text-muted">Of max drawdown</p>
          </div>
        </div>

        {/* Status Badge */}
        <div className="mb-4 flex justify-center">
          <span
            className="px-4 py-2 rounded-full text-sm font-medium text-white"
            style={{ backgroundColor: drawdownStatus.color }}
          >
            {drawdownStatus.status.toUpperCase()} DRAWDOWN
          </span>
        </div>

        {/* Drawdown Chart */}
        <div className="mb-4">
          <h3 className="text-sm font-medium text-text-muted mb-3">Drawdown History</h3>
          <ResponsiveContainer width="100%" height={300}>
            <AreaChart data={chartData}>
              <defs>
                <linearGradient id="drawdownGradient" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="5%" stopColor={COLOR_LOSS} stopOpacity={0.8} />
                  <stop offset="95%" stopColor={COLOR_LOSS} stopOpacity={0} />
                </linearGradient>
              </defs>
              <CartesianGrid strokeDasharray="3 3" stroke={GRID_STROKE} opacity={0.3} vertical={false} />
              <XAxis
                dataKey="timestamp"
                tick={AXIS_TICK}
                stroke={AXIS_STROKE}
                tickLine={false}
                tickFormatter={(value) => {
                  const date = new Date(value)
                  return `${date.getMonth() + 1}/${date.getDate()}`
                }}
              />
              <YAxis
                tick={AXIS_TICK}
                stroke={AXIS_STROKE}
                tickLine={false}
                label={{ value: 'Drawdown %', angle: -90, position: 'insideLeft', fill: '#888888' }}
              />
              <Tooltip content={<CustomTooltip />} />
              <ReferenceLine
                y={currentDrawdownPercent}
                stroke={COLOR_LOSS}
                strokeDasharray="3 3"
                label={{ value: 'Current', fill: COLOR_LOSS, fontSize: 11 }}
              />
              <ReferenceLine
                y={maxDrawdownPercent}
                stroke={COLOR_SPEAR}
                strokeDasharray="3 3"
                label={{ value: 'Max', fill: COLOR_SPEAR, fontSize: 11 }}
              />
              <Area
                type="monotone"
                dataKey="drawdown"
                stroke={COLOR_LOSS}
                strokeWidth={2}
                fillOpacity={1}
                fill="url(#drawdownGradient)"
              />
            </AreaChart>
          </ResponsiveContainer>
        </div>

        {/* Recovery Analysis */}
        {currentDrawdownPercent < maxDrawdownPercent * 0.5 && (
          <div className="p-3 bg-profit/10 border border-profit/30 rounded-lg">
            <p className="text-sm text-profit">
              <strong>Positive Recovery:</strong> Portfolio has recovered {recoveryPercent.toFixed(0)}% from maximum drawdown.
              Current risk level is {drawdownStatus.status.toLowerCase()}.
            </p>
          </div>
        )}

        {currentDrawdownPercent > maxDrawdownPercent * 0.8 && (
          <div className="p-3 bg-loss/10 border border-loss/30 rounded-lg">
            <p className="text-sm text-loss">
              <strong>Elevated Risk:</strong> Portfolio is near maximum drawdown levels.
              Consider reducing position sizes or reviewing risk management strategy.
            </p>
          </div>
        )}
      </CardContent>
    </Card>
  )
}
