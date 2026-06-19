import { BarChart, Bar, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer } from 'recharts'
import type { TradeLatencyResponse } from '../../api'

interface LatencyChartProps {
  data: TradeLatencyResponse
}

export function LatencyChart({ data }: LatencyChartProps) {
  const chartData = data.histogram.map((bucket) => ({
    range: bucket.range,
    count: bucket.count,
    percentage: bucket.percentage,
  }))

  // Show empty state if no data
  if (chartData.length === 0) {
    return (
      <div className="h-64 flex items-center justify-center">
        <div className="text-center text-text-muted">
          <p className="text-sm">No latency data available</p>
          <p className="text-xs mt-1">Charts will populate after trade execution</p>
        </div>
      </div>
    )
  }

  const CustomTooltip = ({ active, payload }: any) => {
    if (active && payload && payload.length) {
      return (
        <div className="bg-surface border border-border rounded-lg p-3 shadow-lg">
          <p className="text-sm font-medium">{payload[0].payload.range}</p>
          <p className="text-xs text-text-muted">Count: {payload[0].payload.count}</p>
          <p className="text-xs text-text-muted">Percentage: {payload[0].payload.percentage.toFixed(1)}%</p>
        </div>
      )
    }
    return null
  }

  return (
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
          <Bar dataKey="count" fill="#3b82f6" radius={[4, 4, 0, 0]} />
        </BarChart>
      </ResponsiveContainer>
    </div>
  )
}
