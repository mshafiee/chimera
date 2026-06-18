import { clsx } from 'clsx'
import { TrendingUp, TrendingDown, Minus } from 'lucide-react'

interface MetricCardProps {
  label: string
  value: string | number
  change?: number | string
  changePercent?: number
  positive?: boolean
  neutral?: boolean
  size?: 'sm' | 'md' | 'lg'
  trend?: 'up' | 'down' | 'neutral'
  unit?: string
  icon?: React.ReactNode
  className?: string
  loading?: boolean
}

export function MetricCard({
  label,
  value,
  change,
  changePercent,
  positive,
  neutral,
  size = 'md',
  trend,
  unit,
  icon,
  className,
  loading = false,
}: MetricCardProps) {
  const sizeClasses = {
    sm: 'text-lg',
    md: 'text-2xl',
    lg: 'text-3xl',
  }

  const labelSizeClasses = {
    sm: 'text-xs',
    md: 'text-sm',
    lg: 'text-base',
  }

  // Determine trend if not explicitly provided
  const determineTrend = (): 'up' | 'down' | 'neutral' => {
    if (trend) return trend
    if (neutral) return 'neutral'
    if (positive !== undefined) return positive ? 'up' : 'down'
    if (changePercent !== undefined) {
      if (Math.abs(changePercent) < 0.01) return 'neutral'
      return changePercent > 0 ? 'up' : 'down'
    }
    return 'neutral'
  }

  const currentTrend = determineTrend()

  const trendIcon = {
    up: <TrendingUp className="w-4 h-4" />,
    down: <TrendingDown className="w-4 h-4" />,
    neutral: <Minus className="w-4 h-4" />,
  }[currentTrend]

  const trendColor = {
    up: 'text-profit',
    down: 'text-loss',
    neutral: 'text-text-muted',
  }[currentTrend]

  const displayValue = loading ? '...' : value

  const formatChange = (val: number | string): string => {
    if (typeof val === 'number') {
      return `${val >= 0 ? '+' : ''}${val.toFixed(2)}%`
    }
    return val
  }

  return (
    <div className={clsx('bg-surface-light rounded-lg p-4', className)}>
      <div className={clsx('text-text-muted mb-1 flex items-center gap-2', labelSizeClasses[size])}>
        {icon && <span className="text-text-muted">{icon}</span>}
        {label}
      </div>
      <div className={clsx('font-semibold font-mono-numbers', sizeClasses[size])}>
        {unit && unit !== '' && !loading ? `${unit} ` : ''}
        {displayValue}
      </div>
      {(change !== undefined || changePercent !== undefined) && !loading && (
        <div className={clsx('text-sm font-mono-numbers flex items-center gap-1 mt-1', trendColor)}>
          {trendIcon}
          {changePercent !== undefined ? formatChange(changePercent) : formatChange(change as number | string)}
        </div>
      )}
    </div>
  )
}
