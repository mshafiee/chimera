import { clsx } from 'clsx'
import { Zap, Radio } from 'lucide-react'

interface RealtimeBadgeProps {
  isLive: boolean
  lastUpdate?: Date | null
  className?: string
  showText?: boolean
  size?: 'sm' | 'md'
}

export function RealtimeBadge({
  isLive,
  lastUpdate,
  className,
  showText = true,
  size = 'md',
}: RealtimeBadgeProps) {
  const sizeClasses = {
    sm: 'text-xs',
    md: 'text-sm',
  }

  const iconSizes = {
    sm: 'w-3 h-3',
    md: 'w-4 h-4',
  }

  const getTimeSinceUpdate = (): string => {
    if (!lastUpdate) return 'Never'

    const now = new Date()
    const diff = now.getTime() - lastUpdate.getTime()

    const seconds = Math.floor(diff / 1000)
    const minutes = Math.floor(seconds / 60)
    const hours = Math.floor(minutes / 60)
    const days = Math.floor(hours / 24)

    if (days > 0) return `${days}d ago`
    if (hours > 0) return `${hours}h ago`
    if (minutes > 0) return `${minutes}m ago`
    if (seconds > 0) return `${seconds}s ago`
    return 'Just now'
  }

  return (
    <div className={clsx('flex items-center gap-2', className)}>
      <div className={clsx('flex items-center gap-1.5', sizeClasses[size])}>
        {isLive ? (
          <>
            <Zap className={clsx('text-profit animate-pulse', iconSizes[size])} />
            <span className="text-profit font-medium">LIVE</span>
          </>
        ) : (
          <>
            <Radio className={clsx('text-text-muted', iconSizes[size])} />
            <span className="text-text-muted">OFFLINE</span>
          </>
        )}
      </div>
      {showText && lastUpdate && (
        <span className={clsx('text-text-muted', sizeClasses[size])}>
          {getTimeSinceUpdate()}
        </span>
      )}
    </div>
  )
}

interface ConnectionStatusProps {
  connected: boolean
  latency?: number
  className?: string
}

export function ConnectionStatus({ connected, latency, className }: ConnectionStatusProps) {
  const getLatencyColor = (): string => {
    if (!latency) return 'text-text-muted'
    if (latency < 50) return 'text-profit'
    if (latency < 100) return 'text-spear'
    return 'text-loss'
  }

  return (
    <div className={clsx('flex items-center gap-2', className)}>
      <div
        className={clsx(
          'w-2 h-2 rounded-full',
          connected ? 'bg-profit animate-pulse' : 'bg-loss'
        )}
      />
      {latency !== undefined && (
        <span className={clsx('text-xs font-mono-numbers', getLatencyColor())}>
          {latency}ms
        </span>
      )}
    </div>
  )
}
