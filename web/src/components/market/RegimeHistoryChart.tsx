import { LineChart, Line, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer, Legend } from 'recharts'
import type { RegimeHistoryPoint } from '../../api'

interface RegimeHistoryChartProps {
  history: RegimeHistoryPoint[]
}

const REGIME_COLORS = {
  bull: '#22c55e',
  bear: '#ef4444',
  neutral: '#6b7280',
  volatile: '#f97316',
}

export function RegimeHistoryChart({ history }: RegimeHistoryChartProps) {
  const chartData = history.map((point) => ({
    time: new Date(point.timestamp).toLocaleDateString('en-US', { month: 'short', day: 'numeric' }),
    volatility: point.volatility_index,
    regime: point.regime,
  }))

  const CustomTooltip = ({ active, payload }: any) => {
    if (active && payload && payload.length) {
      const data = payload[0].payload
      return (
        <div className="bg-surface border border-border rounded-lg p-3 shadow-lg">
          <p className="text-sm font-medium">{data.time}</p>
          <p className="text-xs text-text-muted">Regime: {data.regime}</p>
          <p className="text-xs text-text-muted">Volatility: {data.volatility.toFixed(2)}</p>
        </div>
      )
    }
    return null
  }

  return (
    <div className="h-64">
      <ResponsiveContainer width="100%" height="100%">
        <LineChart data={chartData} margin={{ top: 5, right: 30, left: 20, bottom: 5 }}>
          <CartesianGrid strokeDasharray="3 3" stroke="#374151" opacity={0.3} />
          <XAxis
            dataKey="time"
            tick={{ fill: '#9ca3af', fontSize: 12 }}
            stroke="#6b7280"
          />
          <YAxis
            tick={{ fill: '#9ca3af', fontSize: 12 }}
            stroke="#6b7280"
          />
          <Tooltip content={<CustomTooltip />} />
          <Legend />
          <Line
            type="monotone"
            dataKey="volatility"
            stroke="#f97316"
            strokeWidth={2}
            dot={{ r: 3 }}
            name="Volatility Index"
          />
        </LineChart>
      </ResponsiveContainer>
    </div>
  )
}
