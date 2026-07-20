import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card'
import {
  BarChart,
  Bar,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
  PieChart,
  Pie,
  Cell
} from 'recharts'

interface ConcentrationData {
  name: string
  value: number
  percentage: number
}

interface ConcentrationRiskChartProps {
  byToken: ConcentrationData[]
  bySector: ConcentrationData[]
  maxConcentrationPercent: number
  hhi: number
  className?: string
}

// Theme-aware palette (shield, profit, spear, loss + accents)
const COLORS = ['#00D4FF', '#00FF88', '#FF8800', '#FF4444', '#8B5CF6', '#14B8A6']

const COLOR_PROFIT = '#00FF88'
const COLOR_SPEAR = '#FF8800'
const COLOR_LOSS = '#FF4444'
const COLOR_SHIELD = '#00D4FF'

const AXIS_TICK = { fill: '#888888', fontSize: 12 }
const AXIS_STROKE = '#3A3A3A'
const GRID_STROKE = '#3A3A3A'

export function ConcentrationRiskChart({
  byToken,
  bySector,
  maxConcentrationPercent,
  hhi,
  className = ''
}: ConcentrationRiskChartProps) {
  // HHI ranges: 0-1500 (competitive), 1500-2500 (moderate), 2500+ (high)
  const getHHIStatus = (hhiValue: number) => {
    if (hhiValue < 1500) return { status: 'Competitive', color: COLOR_PROFIT }
    if (hhiValue < 2500) return { status: 'Moderate', color: COLOR_SPEAR }
    return { status: 'High', color: COLOR_LOSS }
  }

  const hhiStatus = getHHIStatus(hhi)

  // Prepare data for token concentration chart
  const tokenData = byToken.slice(0, 10).map(item => ({
    name: item.name.length > 8 ? item.name.substring(0, 8) + '...' : item.name,
    full: item.name,
    value: item.percentage,
    amount: item.value
  }))

  // Prepare data for sector concentration chart
  const sectorData = bySector.map(item => ({
    name: item.name,
    value: item.percentage,
    amount: item.value
  }))

  const TokenTooltip = ({ active, payload }: any) => {
    if (active && payload && payload.length) {
      const data = payload[0].payload
      return (
        <div className="bg-surface border border-border rounded-lg p-3 shadow-lg">
          <p className="font-medium">{data.full}</p>
          <p className="text-xs text-text-muted">Concentration: {data.value.toFixed(1)}%</p>
          <p className="text-xs text-text-muted">Amount: {data.amount.toFixed(2)} SOL</p>
        </div>
      )
    }
    return null
  }

  const SectorTooltip = ({ active, payload }: any) => {
    if (active && payload && payload.length) {
      const data = payload[0].payload
      return (
        <div className="bg-surface border border-border rounded-lg p-3 shadow-lg">
          <p className="font-medium">{data.name}</p>
          <p className="text-xs text-text-muted">Concentration: {data.value.toFixed(1)}%</p>
          <p className="text-xs text-text-muted">Amount: {data.amount.toFixed(2)} SOL</p>
        </div>
      )
    }
    return null
  }

  return (
    <Card className={className}>
      <CardHeader>
        <CardTitle className="text-lg">Concentration Risk Analysis</CardTitle>
      </CardHeader>
      <CardContent>
        {/* HHI Metric */}
        <div className="mb-6 flex items-center justify-between p-4 bg-surface-light rounded-lg">
          <div>
            <h3 className="text-sm font-medium text-text-muted">Herfindahl-Hirschman Index</h3>
            <p className="text-xs text-text-muted">Market concentration measure</p>
          </div>
          <div className="text-right">
            <span className="text-2xl font-bold font-mono-numbers" style={{ color: hhiStatus.color }}>
              {hhi.toFixed(0)}
            </span>
            <span
              className="ml-2 px-2 py-1 text-xs font-medium rounded"
              style={{ backgroundColor: hhiStatus.color + '20', color: hhiStatus.color }}
            >
              {hhiStatus.status}
            </span>
          </div>
        </div>

        {/* Maximum Concentration */}
        <div className="mb-6 p-4 bg-shield/10 rounded-lg">
          <div className="flex justify-between items-center">
            <div>
              <h3 className="text-sm font-medium text-text-muted">Maximum Concentration</h3>
              <p className="text-xs text-text-muted">Largest single position</p>
            </div>
            <span className="text-2xl font-bold font-mono-numbers text-shield">
              {maxConcentrationPercent.toFixed(1)}%
            </span>
          </div>
        </div>

        {/* Charts */}
        <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
          {/* Token Concentration */}
          <div>
            <h3 className="text-sm font-medium text-text-muted mb-3">Top Token Concentrations</h3>
            <ResponsiveContainer width="100%" height={250}>
              <BarChart data={tokenData} layout="vertical">
                <CartesianGrid strokeDasharray="3 3" stroke={GRID_STROKE} opacity={0.3} horizontal={false} />
                <XAxis
                  type="number"
                  domain={[0, Math.max(100, maxConcentrationPercent * 1.2)]}
                  tick={AXIS_TICK}
                  stroke={AXIS_STROKE}
                  tickLine={false}
                />
                <YAxis
                  dataKey="name"
                  type="category"
                  width={80}
                  tick={AXIS_TICK}
                  stroke={AXIS_STROKE}
                  tickLine={false}
                />
                <Tooltip content={<TokenTooltip />} cursor={{ fill: '#2E2E2E', opacity: 0.5 }} />
                <Bar dataKey="value" fill={COLOR_SHIELD} radius={[0, 4, 4, 0]} />
              </BarChart>
            </ResponsiveContainer>
          </div>

          {/* Sector Concentration */}
          <div>
            <h3 className="text-sm font-medium text-text-muted mb-3">Sector Distribution</h3>
            <ResponsiveContainer width="100%" height={250}>
              <PieChart>
                <Pie
                  data={sectorData}
                  cx="50%"
                  cy="50%"
                  labelLine={false}
                  label={({ name, percentage }) => `${name}: ${percentage.toFixed(0)}%`}
                  outerRadius={80}
                  fill={COLOR_SHIELD}
                  dataKey="value"
                  stroke="#242424"
                  strokeWidth={1}
                >
                  {sectorData.map((_entry, index) => (
                    <Cell key={`cell-${index}`} fill={COLORS[index % COLORS.length]} />
                  ))}
                </Pie>
                <Tooltip content={<SectorTooltip />} />
              </PieChart>
            </ResponsiveContainer>
          </div>
        </div>

        {/* Risk Alert */}
        {maxConcentrationPercent > 20 && (
          <div className="mt-4 p-3 bg-spear/10 border border-spear/30 rounded-lg">
            <p className="text-sm text-spear">
              <strong>Warning:</strong> High concentration detected. Consider diversifying positions to reduce risk.
            </p>
          </div>
        )}
      </CardContent>
    </Card>
  )
}
