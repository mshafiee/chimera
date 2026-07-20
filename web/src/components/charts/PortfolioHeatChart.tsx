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

// Theme tokens (see tailwind.config.js)
const COLOR_PROFIT = '#00FF88' // profit (green)
const COLOR_SPEAR = '#FF8800' // spear (orange)
const COLOR_LOSS = '#FF4444' // loss (red)
const COLOR_MUTED = '#888888' // text-muted (gray)
const COLOR_TRACK = '#3A3A3A' // border (track for remaining)

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
        return COLOR_LOSS
      case 'elevated':
        return COLOR_SPEAR
      default:
        return COLOR_PROFIT
    }
  }

  const currentColor = getStatusColor(heatStatus)
  const thresholdColor = COLOR_MUTED

  // Create data for the pie chart
  const data: PortfolioHeatData[] = [
    { name: 'Current Heat', value: heatPercentage, color: currentColor },
    { name: 'Remaining', value: 100 - heatPercentage, color: COLOR_TRACK }
  ]

  // Threshold indicator
  const thresholdData: PortfolioHeatData[] = [
    { name: 'Threshold', value: heatThreshold, color: thresholdColor },
    { name: 'Safe Zone', value: 100 - heatThreshold, color: COLOR_TRACK }
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
            <h3 className="text-sm font-medium text-text-muted mb-2">Current Heat Level</h3>
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
                    <Cell key={`cell-${index}`} fill={entry.color} stroke="#242424" strokeWidth={1} />
                  ))}
                </Pie>
                <Tooltip
                  contentStyle={{
                    backgroundColor: '#242424',
                    border: '1px solid #3A3A3A',
                    borderRadius: '8px',
                    color: '#E0E0E0',
                    fontSize: '12px',
                  }}
                  formatter={(value: number) => [`${value.toFixed(1)}%`, 'Heat']}
                />
              </PieChart>
            </ResponsiveContainer>
            <div className="text-center mt-2">
              <span className="text-2xl font-bold font-mono-numbers" style={{ color: currentColor }}>
                {heatPercentage.toFixed(1)}%
              </span>
              <p className="text-xs text-text-muted">Current Heat</p>
            </div>
          </div>

          {/* Threshold */}
          <div>
            <h3 className="text-sm font-medium text-text-muted mb-2">Heat Threshold</h3>
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
                    <Cell key={`cell-${index}`} fill={entry.color} stroke="#242424" strokeWidth={1} />
                  ))}
                </Pie>
                <Tooltip
                  contentStyle={{
                    backgroundColor: '#242424',
                    border: '1px solid #3A3A3A',
                    borderRadius: '8px',
                    color: '#E0E0E0',
                    fontSize: '12px',
                  }}
                  formatter={(value: number) => [`${value.toFixed(1)}%`, 'Threshold']}
                />
              </PieChart>
            </ResponsiveContainer>
            <div className="text-center mt-2">
              <span className="text-2xl font-bold font-mono-numbers" style={{ color: thresholdColor }}>
                {heatThreshold.toFixed(1)}%
              </span>
              <p className="text-xs text-text-muted">Warning Threshold</p>
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
