import { Badge } from '../ui/Badge'
import type { PortfolioRiskResponse } from '../../api'

interface PortfolioHeatGaugeProps {
  data: PortfolioRiskResponse
}

export function PortfolioHeatGauge({ data }: PortfolioHeatGaugeProps) {
  const heatPercent = data.portfolio_heat_percent
  const threshold = data.heat_threshold
  const status = data.heat_status

  // Determine color based on status
  const statusColor = {
    normal: '#22c55e',
    elevated: '#f97316',
    high: '#ef4444',
    critical: '#dc2626',
  }[status]

  const gaugeSegments = [
    { limit: 50, color: '#22c55e' },      // Normal (green)
    { limit: 70, color: '#f97316' },     // Elevated (orange)
    { limit: 85, color: '#ef4444' },     // High (red)
    { limit: 100, color: '#dc2626' },   // Critical (dark red)
  ]

  return (
    <div className="space-y-6">
      {/* Gauge Display */}
      <div className="flex items-center justify-center">
        <div className="relative w-64 h-32 overflow-hidden">
          {/* Background Gauge */}
          <svg viewBox="0 0 200 100" className="w-full h-full">
            {/* Background arc */}
            <path
              d="M 20 100 A 80 80 0 0 1 180 100"
              fill="none"
              stroke="#374151"
              strokeWidth="20"
              strokeLinecap="round"
            />
            {/* Colored segments */}
            {gaugeSegments.map(() => {
              // Simplified - just show a single colored arc for current level
              return null
            })}
            {/* Current level arc */}
            <path
              d="M 20 100 A 80 80 0 0 1 180 100"
              fill="none"
              stroke={statusColor}
              strokeWidth="20"
              strokeLinecap="round"
              strokeDasharray={`${(heatPercent / 100) * 251.2} 251.2`}
            />
          </svg>
          {/* Center text */}
          <div className="absolute bottom-0 left-1/2 transform -translate-x-1/2 text-center">
            <div className={`text-3xl font-bold font-mono-numbers`} style={{ color: statusColor }}>
              {heatPercent.toFixed(1)}%
            </div>
            <div className="text-xs text-text-muted mt-1">Portfolio Heat</div>
          </div>
        </div>
      </div>

      {/* Status Badge */}
      <div className="flex justify-center">
        <Badge
          variant={
            status === 'normal' ? 'success' :
            status === 'elevated' ? 'warning' :
            status === 'high' ? 'danger' : 'danger'
          }
          size="md"
        >
          {status.charAt(0).toUpperCase() + status.slice(1)}
        </Badge>
      </div>

      {/* Summary Stats */}
      <div className="grid grid-cols-3 gap-4 pt-4 border-t border-border">
        <div className="text-center">
          <div className="text-xl font-semibold font-mono-numbers">{threshold.toFixed(0)}%</div>
          <div className="text-xs text-text-muted">Threshold</div>
        </div>
        <div className="text-center">
          <div className="text-xl font-semibold font-mono-numbers">
            {data.exposure.total_exposure_sol.toFixed(2)} SOL
          </div>
          <div className="text-xs text-text-muted">Total Exposure</div>
        </div>
        <div className="text-center">
          <div className="text-xl font-semibold font-mono-numbers">
            {data.concentration.max_concentration_percent.toFixed(1)}%
          </div>
          <div className="text-xs text-text-muted">Max Concentration</div>
        </div>
      </div>
    </div>
  )
}
