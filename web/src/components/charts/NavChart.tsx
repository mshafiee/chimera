import {
  AreaChart,
  Area,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
  ReferenceLine,
} from 'recharts'

interface NavDataPoint {
  /** Short label for the X axis. */
  time: string
  /** NAV in SOL. */
  nav: number
  /** Capital baseline in SOL (for a reference line / context). */
  capital: number
}

interface NavChartProps {
  data: NavDataPoint[]
  /** Starting capital (SOL); drawn as a reference line. */
  startCapital?: number
}

const SOL = (value: number) => `${value.toFixed(3)} SOL`

export function NavChart({ data, startCapital }: NavChartProps) {
  const isPositive = data.length > 0 && data[data.length - 1].nav >= (startCapital ?? data[0].nav)
  const ref = startCapital ?? data[0]?.capital

  return (
    <ResponsiveContainer width="100%" height={200}>
      <AreaChart data={data} margin={{ top: 10, right: 10, left: 0, bottom: 0 }}>
        <defs>
          <linearGradient id="colorNavPositive" x1="0" y1="0" x2="0" y2="1">
            <stop offset="5%" stopColor="#00FF88" stopOpacity={0.3} />
            <stop offset="95%" stopColor="#00FF88" stopOpacity={0} />
          </linearGradient>
          <linearGradient id="colorNavNegative" x1="0" y1="0" x2="0" y2="1">
            <stop offset="5%" stopColor="#FF4444" stopOpacity={0.3} />
            <stop offset="95%" stopColor="#FF4444" stopOpacity={0} />
          </linearGradient>
        </defs>
        <CartesianGrid strokeDasharray="3 3" stroke="#3A3A3A" vertical={false} />
        <XAxis
          dataKey="time"
          stroke="#888888"
          tick={{ fill: '#888888', fontSize: 12 }}
          axisLine={{ stroke: '#3A3A3A' }}
          tickLine={false}
          minTickGap={24}
        />
        <YAxis
          stroke="#888888"
          tick={{ fill: '#888888', fontSize: 12 }}
          axisLine={{ stroke: '#3A3A3A' }}
          tickLine={false}
          domain={['auto', 'auto']}
          tickFormatter={(value) => SOL(Number(value))}
          width={84}
        />
        <Tooltip
          contentStyle={{
            backgroundColor: '#242424',
            border: '1px solid #3A3A3A',
            borderRadius: '8px',
            color: '#E0E0E0',
          }}
          formatter={(value: number, name: string) => [
            SOL(Number(value)),
            name === 'nav' ? 'NAV' : name,
          ]}
          labelFormatter={(label) => `Time: ${label}`}
        />
        {ref !== undefined && (
          <ReferenceLine
            y={ref}
            stroke="#888888"
            strokeDasharray="4 4"
            label={{ value: `start ${SOL(ref)}`, fill: '#888888', fontSize: 10, position: 'insideTopRight' }}
          />
        )}
        <Area
          type="monotone"
          dataKey="nav"
          stroke={isPositive ? '#00FF88' : '#FF4444'}
          strokeWidth={2}
          fillOpacity={1}
          fill={isPositive ? 'url(#colorNavPositive)' : 'url(#colorNavNegative)'}
          isAnimationActive={false}
        />
      </AreaChart>
    </ResponsiveContainer>
  )
}
