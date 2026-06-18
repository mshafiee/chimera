import { AreaChart, Area, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer } from 'recharts'
import type { SignalQualityResponse } from '../../api'

interface SignalQualityChartProps {
  data: SignalQualityResponse
}

export function SignalQualityChart({ data }: SignalQualityChartProps) {
  const chartData = data.quality_distribution.map((bucket) => ({
    range: bucket.range,
    count: bucket.count,
    percentage: bucket.percentage,
  }))

  const trendData = data.average_quality_trend.map((point) => ({
    time: new Date(point.timestamp).toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit' }),
    score: point.average_score,
  }))

  const CustomTooltip = ({ active, payload, label }: any) => {
    if (active && payload && payload.length) {
      return (
        <div className="bg-surface border border-border rounded-lg p-3 shadow-lg">
          <p className="text-sm font-medium">{label}</p>
          <p className="text-xs text-text-muted">Count: {payload[0].payload.count}</p>
          <p className="text-xs text-text-muted">Percentage: {payload[0].payload.percentage.toFixed(1)}%</p>
        </div>
      )
    }
    return null
  }

  return (
    <div className="space-y-6">
      {/* Distribution Chart */}
      <div>
        <h3 className="text-sm font-medium mb-3">Quality Distribution</h3>
        <div className="h-48">
          <ResponsiveContainer width="100%" height="100%">
            <AreaChart data={chartData} margin={{ top: 5, right: 30, left: 20, bottom: 5 }}>
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
              <Area
                type="monotone"
                dataKey="count"
                stroke="#22c55e"
                fill="#22c55e"
                fillOpacity={0.3}
              />
            </AreaChart>
          </ResponsiveContainer>
        </div>
      </div>

      {/* Quality Trend */}
      {trendData.length > 0 && (
        <div>
          <h3 className="text-sm font-medium mb-3">Average Quality Trend</h3>
          <div className="h-48">
            <ResponsiveContainer width="100%" height="100%">
              <AreaChart data={trendData} margin={{ top: 5, right: 30, left: 20, bottom: 5 }}>
                <CartesianGrid strokeDasharray="3 3" stroke="#374151" opacity={0.3} />
                <XAxis
                  dataKey="time"
                  tick={{ fill: '#9ca3af', fontSize: 12 }}
                  stroke="#6b7280"
                />
                <YAxis
                  domain={[0, 1]}
                  tick={{ fill: '#9ca3af', fontSize: 12 }}
                  stroke="#6b7280"
                />
                <Tooltip
                  content={({ active, payload }: any) => {
                    if (active && payload && payload.length) {
                      return (
                        <div className="bg-surface border border-border rounded-lg p-3 shadow-lg">
                          <p className="text-sm font-medium">{payload[0].payload.time}</p>
                          <p className="text-xs text-text-muted">Score: {payload[0].payload.score.toFixed(2)}</p>
                        </div>
                      )
                    }
                    return null
                  }}
                />
                <Area
                  type="monotone"
                  dataKey="score"
                  stroke="#3b82f6"
                  fill="#3b82f6"
                  fillOpacity={0.3}
                />
              </AreaChart>
            </ResponsiveContainer>
          </div>
        </div>
      )}

      {/* Summary Stats */}
      <div className="grid grid-cols-2 gap-4 pt-4 border-t border-border">
        <div>
          <div className="text-2xl font-bold font-mono-numbers">{(data.rejection_rate * 100).toFixed(1)}%</div>
          <div className="text-xs text-text-muted">Rejection Rate</div>
        </div>
        <div>
          <div className="text-2xl font-bold font-mono-numbers">
            {data.total_signals > 0 ? (data.accepted_signals / data.total_signals * 100).toFixed(1) : 0}%
          </div>
          <div className="text-xs text-text-muted">Acceptance Rate</div>
        </div>
      </div>
    </div>
  )
}
