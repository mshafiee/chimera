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
    if (ratio < 0.3) return { status: 'Minor', color: '#10b981' }
    if (ratio < 0.6) return { status: 'Moderate', color: '#f59e0b' }
    return { status: 'Significant', color: '#ef4444' }
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

  return (
    <Card className={className}>
      <CardHeader>
        <CardTitle className="text-lg">Drawdown Analysis</CardTitle>
      </CardHeader>
      <CardContent>
        {/* Key Metrics */}
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4 mb-6">
          {/* Current Drawdown */}
          <div className="p-4 bg-red-50 rounded-lg">
            <h3 className="text-sm font-medium text-gray-600">Current Drawdown</h3>
            <p className="text-2xl font-bold text-red-600">
              {currentDrawdownPercent.toFixed(1)}%
            </p>
            <p className="text-xs text-gray-500">From peak</p>
          </div>

          {/* Maximum Drawdown */}
          <div className="p-4 bg-orange-50 rounded-lg">
            <h3 className="text-sm font-medium text-gray-600">Max Drawdown</h3>
            <p className="text-2xl font-bold text-orange-600">
              {maxDrawdownPercent.toFixed(1)}%
            </p>
            <p className="text-xs text-gray-500">Historical worst</p>
          </div>

          {/* Duration */}
          <div className="p-4 bg-blue-50 rounded-lg">
            <h3 className="text-sm font-medium text-gray-600">Drawdown Duration</h3>
            <p className="text-2xl font-bold text-blue-600">
              {drawdownDurationDays}
            </p>
            <p className="text-xs text-gray-500">Days since peak</p>
          </div>

          {/* Recovery Progress */}
          <div className="p-4 bg-green-50 rounded-lg">
            <h3 className="text-sm font-medium text-gray-600">Recovery Progress</h3>
            <p className="text-2xl font-bold text-green-600">
              {recoveryPercent.toFixed(0)}%
            </p>
            <p className="text-xs text-gray-500">Of max drawdown</p>
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
          <h3 className="text-sm font-medium text-gray-600 mb-3">Drawdown History</h3>
          <ResponsiveContainer width="100%" height={300}>
            <AreaChart data={chartData}>
              <defs>
                <linearGradient id="drawdownGradient" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="5%" stopColor="#ef4444" stopOpacity={0.8} />
                  <stop offset="95%" stopColor="#ef4444" stopOpacity={0} />
                </linearGradient>
              </defs>
              <CartesianGrid strokeDasharray="3 3" />
              <XAxis
                dataKey="timestamp"
                tickFormatter={(value) => {
                  const date = new Date(value)
                  return `${date.getMonth() + 1}/${date.getDate()}`
                }}
              />
              <YAxis label={{ value: 'Drawdown %', angle: -90, position: 'insideLeft' }} />
              <Tooltip
                formatter={(value: number) => [`${value.toFixed(1)}%`, 'Drawdown']}
                labelFormatter={(value) => new Date(value).toLocaleDateString()}
              />
              <ReferenceLine
                y={currentDrawdownPercent}
                stroke="#ef4444"
                strokeDasharray="3 3"
                label="Current"
              />
              <ReferenceLine
                y={maxDrawdownPercent}
                stroke="#f59e0b"
                strokeDasharray="3 3"
                label="Max"
              />
              <Area
                type="monotone"
                dataKey="drawdown"
                stroke="#ef4444"
                fillOpacity={1}
                fill="url(#drawdownGradient)"
              />
            </AreaChart>
          </ResponsiveContainer>
        </div>

        {/* Recovery Analysis */}
        {currentDrawdownPercent < maxDrawdownPercent * 0.5 && (
          <div className="p-3 bg-green-50 border border-green-200 rounded-lg">
            <p className="text-sm text-green-800">
              <strong>Positive Recovery:</strong> Portfolio has recovered {recoveryPercent.toFixed(0)}% from maximum drawdown.
              Current risk level is {drawdownStatus.status.toLowerCase()}.
            </p>
          </div>
        )}

        {currentDrawdownPercent > maxDrawdownPercent * 0.8 && (
          <div className="p-3 bg-red-50 border border-red-200 rounded-lg">
            <p className="text-sm text-red-800">
              <strong>Elevated Risk:</strong> Portfolio is near maximum drawdown levels.
              Consider reducing position sizes or reviewing risk management strategy.
            </p>
          </div>
        )}
      </CardContent>
    </Card>
  )
}