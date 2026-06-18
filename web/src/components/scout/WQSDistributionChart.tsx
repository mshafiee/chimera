import { BarChart, Bar, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer, Cell } from 'recharts'
import type { WQSDistributionResponse } from '../../api'

interface WQSDistributionChartProps {
  data: WQSDistributionResponse
}

const WQS_RANGES = [
  { min: 0, max: 20, color: '#ef4444' },    // red
  { min: 20, max: 40, color: '#f97316' },   // orange
  { min: 40, max: 60, color: '#eab308' },   // yellow
  { min: 60, max: 80, color: '#22c55e' },   // green
  { min: 80, max: 100, color: '#3b82f6' },  // blue
]

export function WQSDistributionChart({ data }: WQSDistributionChartProps) {
  // Prepare chart data
  const chartData = data.distribution.map((bucket) => ({
    range: bucket.range,
    count: bucket.count,
    percentage: bucket.percentage,
  }))

  const getColor = (range: string): string => {
    const rangeObj = WQS_RANGES.find((r) => range.startsWith(r.min.toString()))
    return rangeObj?.color || '#6b7280'
  }

  const CustomTooltip = ({ active, payload }: any) => {
    if (active && payload && payload.length) {
      const data = payload[0].payload
      return (
        <div className="bg-surface border border-border rounded-lg p-3 shadow-lg">
          <p className="text-sm font-medium">WQS: {data.range}</p>
          <p className="text-xs text-text-muted">Count: {data.count}</p>
          <p className="text-xs text-text-muted">Percentage: {data.percentage.toFixed(1)}%</p>
        </div>
      )
    }
    return null
  }

  return (
    <div className="space-y-4">
      {/* Summary Stats */}
      <div className="grid grid-cols-3 gap-4 text-center">
        <div>
          <div className="text-2xl font-bold font-mono-numbers">{data.average_score.toFixed(1)}</div>
          <div className="text-xs text-text-muted">Average Score</div>
        </div>
        <div>
          <div className="text-2xl font-bold font-mono-numbers">{data.median_score.toFixed(1)}</div>
          <div className="text-xs text-text-muted">Median Score</div>
        </div>
        <div>
          <div className="text-2xl font-bold font-mono-numbers">{data.total_wallets}</div>
          <div className="text-xs text-text-muted">Total Wallets</div>
        </div>
      </div>

      {/* Chart */}
      <div className="h-64">
        <ResponsiveContainer width="100%" height="100%">
          <BarChart data={chartData} margin={{ top: 5, right: 30, left: 20, bottom: 5 }}>
            <CartesianGrid strokeDasharray="3 3" stroke="#374151" opacity={0.3} />
            <XAxis
              dataKey="range"
              tick={{ fill: '#9ca3af', fontSize: 12 }}
              stroke="#6b7280"
            />
            <YAxis
              tick={{ fill: '#9ca3af', fontSize: 12 }}
              stroke="#6b7280"
            />
            <Tooltip content={<CustomTooltip />} />
            <Bar dataKey="count" radius={[4, 4, 0, 0]}>
              {chartData.map((entry, index) => (
                <Cell key={`cell-${index}`} fill={getColor(entry.range)} />
              ))}
            </Bar>
          </BarChart>
        </ResponsiveContainer>
      </div>
    </div>
  )
}
