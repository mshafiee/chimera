import { Badge } from '../ui/Badge'
import { TrendingUp, TrendingDown, Minus } from 'lucide-react'
import type { MarketRegimeResponse } from '../../api'

interface RegimeIndicatorProps {
  data: MarketRegimeResponse
}

const REGIME_CONFIG = {
  bull: {
    label: 'Bull Market',
    color: 'text-profit',
    bgColor: 'bg-profit/10',
    borderColor: 'border-profit/30',
    icon: TrendingUp,
  },
  bear: {
    label: 'Bear Market',
    color: 'text-loss',
    bgColor: 'bg-loss/10',
    borderColor: 'border-loss/30',
    icon: TrendingDown,
  },
  neutral: {
    label: 'Neutral Market',
    color: 'text-text-muted',
    bgColor: 'bg-surface-light',
    borderColor: 'border-border',
    icon: Minus,
  },
  volatile: {
    label: 'Volatile Market',
    color: 'text-spear',
    bgColor: 'bg-spear/10',
    borderColor: 'border-spear/30',
    icon: Activity,
  },
}

export function RegimeIndicator({ data }: RegimeIndicatorProps) {
  const config = REGIME_CONFIG[data.current_regime]
  const Icon = config.icon

  return (
    <div className={`flex items-center gap-4 p-6 rounded-lg border ${config.bgColor} ${config.borderColor}`}>
      <div className={`p-4 rounded-full ${config.bgColor}`}>
        <Icon className={`w-8 h-8 ${config.color}`} />
      </div>
      <div className="flex-1">
        <div className="text-2xl font-bold">{config.label}</div>
        <div className="text-sm text-text-muted mt-1">
          Confidence: {(data.confidence * 100).toFixed(1)}%
        </div>
      </div>
      <div className="text-right">
        <div className="text-sm text-text-muted">Last Change</div>
        <div className="text-sm font-medium">
          {new Date(data.last_regime_change).toLocaleString()}
        </div>
      </div>
    </div>
  )
}
