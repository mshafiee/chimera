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

const COLORS = ['#0088FE', '#00C49F', '#FFBB28', '#FF8042', '#8884D8', '#82CA9D']

export function ConcentrationRiskChart({
  byToken,
  bySector,
  maxConcentrationPercent,
  hhi,
  className = ''
}: ConcentrationRiskChartProps) {
  // HHI ranges: 0-1500 (competitive), 1500-2500 (moderate), 2500+ (high)
  const getHHIStatus = (hhiValue: number) => {
    if (hhiValue < 1500) return { status: 'Competitive', color: '#10b981' }
    if (hhiValue < 2500) return { status: 'Moderate', color: '#f59e0b' }
    return { status: 'High', color: '#ef4444' }
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

  return (
    <Card className={className}>
      <CardHeader>
        <CardTitle className="text-lg">Concentration Risk Analysis</CardTitle>
      </CardHeader>
      <CardContent>
        {/* HHI Metric */}
        <div className="mb-6 flex items-center justify-between p-4 bg-gray-50 rounded-lg">
          <div>
            <h3 className="text-sm font-medium text-gray-600">Herfindahl-Hirschman Index</h3>
            <p className="text-xs text-gray-500">Market concentration measure</p>
          </div>
          <div className="text-right">
            <span className="text-2xl font-bold" style={{ color: hhiStatus.color }}>
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
        <div className="mb-6 p-4 bg-blue-50 rounded-lg">
          <div className="flex justify-between items-center">
            <div>
              <h3 className="text-sm font-medium text-gray-600">Maximum Concentration</h3>
              <p className="text-xs text-gray-500">Largest single position</p>
            </div>
            <span className="text-2xl font-bold text-blue-600">
              {maxConcentrationPercent.toFixed(1)}%
            </span>
          </div>
        </div>

        {/* Charts */}
        <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
          {/* Token Concentration */}
          <div>
            <h3 className="text-sm font-medium text-gray-600 mb-3">Top Token Concentrations</h3>
            <ResponsiveContainer width="100%" height={250}>
              <BarChart data={tokenData} layout="vertical">
                <CartesianGrid strokeDasharray="3 3" />
                <XAxis type="number" domain={[0, Math.max(100, maxConcentrationPercent * 1.2)]} />
                <YAxis dataKey="name" type="category" width={80} />
                <Tooltip
                  formatter={(value: number, name: string) => {
                    if (name === 'value') {
                      return [`${value.toFixed(1)}%`, 'Concentration']
                    }
                    return [value, name]
                  }}
                  content={({ payload }) => {
                    if (payload && payload.length > 0) {
                      const data = payload[0].payload
                      return (
                        <div className="bg-white p-2 border rounded shadow">
                          <p className="font-medium">{data.full}</p>
                          <p>Concentration: {data.value.toFixed(1)}%</p>
                          <p>Amount: {data.amount.toFixed(2)} SOL</p>
                        </div>
                      )
                    }
                    return null
                  }}
                />
                <Bar dataKey="value" fill="#3b82f6" radius={[0, 4, 4, 0]} />
              </BarChart>
            </ResponsiveContainer>
          </div>

          {/* Sector Concentration */}
          <div>
            <h3 className="text-sm font-medium text-gray-600 mb-3">Sector Distribution</h3>
            <ResponsiveContainer width="100%" height={250}>
              <PieChart>
                <Pie
                  data={sectorData}
                  cx="50%"
                  cy="50%"
                  labelLine={false}
                  label={({ name, percentage }) => `${name}: ${percentage.toFixed(0)}%`}
                  outerRadius={80}
                  fill="#8884d8"
                  dataKey="value"
                >
                  {sectorData.map((_entry, index) => (
                    <Cell key={`cell-${index}`} fill={COLORS[index % COLORS.length]} />
                  ))}
                </Pie>
                <Tooltip
                  content={({ payload }) => {
                    if (payload && payload.length > 0) {
                      const data = payload[0].payload
                      return (
                        <div className="bg-white p-2 border rounded shadow">
                          <p className="font-medium">{data.name}</p>
                          <p>Concentration: {data.value.toFixed(1)}%</p>
                          <p>Amount: {data.amount.toFixed(2)} SOL</p>
                        </div>
                      )
                    }
                    return null
                  }}
                />
              </PieChart>
            </ResponsiveContainer>
          </div>
        </div>

        {/* Risk Alert */}
        {maxConcentrationPercent > 20 && (
          <div className="mt-4 p-3 bg-yellow-50 border border-yellow-200 rounded-lg">
            <p className="text-sm text-yellow-800">
              <strong>Warning:</strong> High concentration detected. Consider diversifying positions to reduce risk.
            </p>
          </div>
        )}
      </CardContent>
    </Card>
  )
}