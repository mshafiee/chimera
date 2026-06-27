import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card'
import {
  Pie,
  PieChart,
  Cell,
  ResponsiveContainer,
  Tooltip
} from 'recharts'

interface PortfolioHeatData {
  name: string
  value: number
  color: string
}

interface PortfolioHeatChartProps {
  heatPercentage: number
  heatThreshold: number
  heatStatus: 'normal' | 'elevated' | 'critical'
  className?: string
}

export function PortfolioHeatChart({
  heatPercentage,
  heatThreshold,
  heatStatus,
  className = ''
}: PortfolioHeatChartProps) {
  // Determine color based on status
  const getStatusColor = (status: string) => {
    switch (status) {
      case 'critical':
        return '#ef4444' // red
      case 'elevated':
        return '#f59e0b' // amber
      default:
        return '#10b981' // green
    }
  }

  const currentColor = getStatusColor(heatStatus)
  const thresholdColor = '#6b7280' // gray for threshold

  // Create data for the pie chart
  const data: PortfolioHeatData[] = [
    { name: 'Current Heat', value: heatPercentage, color: currentColor },
    { name: 'Remaining', value: 100 - heatPercentage, color: '#f3f4f6' }
  ]

  // Threshold indicator
  const thresholdData: PortfolioHeatData[] = [
    { name: 'Threshold', value: heatThreshold, color: thresholdColor },
    { name: 'Safe Zone', value: 100 - heatThreshold, color: '#f3f4f6' }
  ]

  return (
    <Card className={className}>
      <CardHeader>
        <CardTitle className="text-lg">Portfolio Heat Status</CardTitle>
      </CardHeader>
      <CardContent>
        <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
          {/* Current Heat */}
          <div>
            <h3 className="text-sm font-medium text-gray-600 mb-2">Current Heat Level</h3>
            <ResponsiveContainer width="100%" height={150}>
              <PieChart>
                <Pie
                  data={data}
                  cx="50%"
                  cy="50%"
                  startAngle={90}
                  endAngle={-270}
                  innerRadius={40}
                  outerRadius={60}
                  paddingAngle={2}
                  dataKey="value"
                >
                  {data.map((entry, index) => (
                    <Cell key={`cell-${index}`} fill={entry.color} />
                  ))}
                </Pie>
                <Tooltip />
              </PieChart>
            </ResponsiveContainer>
            <div className="text-center mt-2">
              <span className="text-2xl font-bold" style={{ color: currentColor }}>
                {heatPercentage.toFixed(1)}%
              </span>
              <p className="text-xs text-gray-500">Current Heat</p>
            </div>
          </div>

          {/* Threshold */}
          <div>
            <h3 className="text-sm font-medium text-gray-600 mb-2">Heat Threshold</h3>
            <ResponsiveContainer width="100%" height={150}>
              <PieChart>
                <Pie
                  data={thresholdData}
                  cx="50%"
                  cy="50%"
                  startAngle={90}
                  endAngle={-270}
                  innerRadius={40}
                  outerRadius={60}
                  paddingAngle={2}
                  dataKey="value"
                >
                  {thresholdData.map((entry, index) => (
                    <Cell key={`cell-${index}`} fill={entry.color} />
                  ))}
                </Pie>
                <Tooltip />
              </PieChart>
            </ResponsiveContainer>
            <div className="text-center mt-2">
              <span className="text-2xl font-bold" style={{ color: thresholdColor }}>
                {heatThreshold.toFixed(1)}%
              </span>
              <p className="text-xs text-gray-500">Warning Threshold</p>
            </div>
          </div>
        </div>

        {/* Status Badge */}
        <div className="mt-4 flex justify-center">
          <span
            className="px-4 py-2 rounded-full text-sm font-medium text-white"
            style={{ backgroundColor: currentColor }}
          >
            {heatStatus.toUpperCase()}
          </span>
        </div>
      </CardContent>
    </Card>
  )
}