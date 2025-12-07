import {
  AreaChart,
  Area,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
} from 'recharts'

interface PnLDataPoint {
  date: string
  pnl: number
}

interface PnLChartProps {
  data: PnLDataPoint[]
}

export function PnLChart({ data }: PnLChartProps) {
  // Determine if overall trend is positive
  const isPositive = data.length > 0 && data[data.length - 1].pnl >= 0

  return (
    <ResponsiveContainer width="100%" height={200}>
      <AreaChart data={data} margin={{ top: 10, right: 10, left: 0, bottom: 0 }}>
        <defs>
          <linearGradient id="colorPnlPositive" x1="0" y1="0" x2="0" y2="1">
            <stop offset="5%" stopColor="#00FF88" stopOpacity={0.3} />
            <stop offset="95%" stopColor="#00FF88" stopOpacity={0} />
          </linearGradient>
          <linearGradient id="colorPnlNegative" x1="0" y1="0" x2="0" y2="1">
            <stop offset="5%" stopColor="#FF4444" stopOpacity={0.3} />
            <stop offset="95%" stopColor="#FF4444" stopOpacity={0} />
          </linearGradient>
        </defs>
        <CartesianGrid strokeDasharray="3 3" stroke="#3A3A3A" vertical={false} />
        <XAxis
          dataKey="date"
          stroke="#888888"
          tick={{ fill: '#888888', fontSize: 12 }}
          axisLine={{ stroke: '#3A3A3A' }}
          tickLine={false}
        />
        <YAxis
          stroke="#888888"
          tick={{ fill: '#888888', fontSize: 12 }}
          axisLine={{ stroke: '#3A3A3A' }}
          tickLine={false}
          tickFormatter={(value) => `$${value}`}
        />
        <Tooltip
          contentStyle={{
            backgroundColor: '#242424',
            border: '1px solid #3A3A3A',
            borderRadius: '8px',
            color: '#E0E0E0',
          }}
          formatter={(value: number) => [`$${value.toFixed(2)}`, 'PnL']}
          labelFormatter={(label) => `Date: ${label}`}
        />
        <Area
          type="monotone"
          dataKey="pnl"
          stroke={isPositive ? '#00FF88' : '#FF4444'}
          strokeWidth={2}
          fillOpacity={1}
          fill={isPositive ? 'url(#colorPnlPositive)' : 'url(#colorPnlNegative)'}
        />
      </AreaChart>
    </ResponsiveContainer>
  )
}

// Generate sample data for demo
export function generateSamplePnLData(days: number = 30): PnLDataPoint[] {
  const data: PnLDataPoint[] = []
  let cumPnl = 0

  for (let i = days; i >= 0; i--) {
    const date = new Date()
    date.setDate(date.getDate() - i)
    
    // Random daily change between -50 and +100
    const dailyChange = (Math.random() - 0.4) * 150
    cumPnl += dailyChange

    data.push({
      date: date.toLocaleDateString('en-US', { month: 'short', day: 'numeric' }),
      pnl: Math.round(cumPnl * 100) / 100,
    })
  }

  return data
}
