import { HTMLAttributes, forwardRef } from 'react'
import { clsx } from 'clsx'

interface BadgeProps extends HTMLAttributes<HTMLSpanElement> {
  variant?: 'default' | 'success' | 'warning' | 'danger' | 'shield' | 'spear' | 'info'
  size?: 'sm' | 'md'
}

export const Badge = forwardRef<HTMLSpanElement, BadgeProps>(
  ({ className, variant = 'default', size = 'sm', children, ...props }, ref) => {
    return (
      <span
        ref={ref}
        className={clsx(
          'inline-flex items-center font-medium rounded-full',
          // Variants
          {
            'bg-surface-light text-text': variant === 'default',
            'bg-profit/20 text-profit': variant === 'success',
            'bg-spear/20 text-spear': variant === 'warning',
            'bg-loss/20 text-loss': variant === 'danger',
            'bg-shield/20 text-shield': variant === 'shield',
            'bg-orange-500/20 text-orange-400': variant === 'spear',
            'bg-blue-500/20 text-blue-400': variant === 'info',
          },
          // Sizes
          {
            'text-xs px-2 py-0.5': size === 'sm',
            'text-sm px-2.5 py-1': size === 'md',
          },
          className
        )}
        {...props}
      >
        {children}
      </span>
    )
  }
)

Badge.displayName = 'Badge'

// Status-specific badges
export function StatusBadge({ status }: { status: string }) {
  const statusConfig: Record<string, { variant: BadgeProps['variant']; label: string }> = {
    ACTIVE: { variant: 'success', label: 'Active' },
    EXITING: { variant: 'warning', label: 'Exiting' },
    CLOSED: { variant: 'default', label: 'Closed' },
    PENDING: { variant: 'info', label: 'Pending' },
    QUEUED: { variant: 'info', label: 'Queued' },
    EXECUTING: { variant: 'warning', label: 'Executing' },
    FAILED: { variant: 'danger', label: 'Failed' },
    RETRY: { variant: 'warning', label: 'Retry' },
    DEAD_LETTER: { variant: 'danger', label: 'Dead Letter' },
    CANDIDATE: { variant: 'info', label: 'Candidate' },
    REJECTED: { variant: 'danger', label: 'Rejected' },
  }

  const config = statusConfig[status] || { variant: 'default', label: status }

  return <Badge variant={config.variant}>{config.label}</Badge>
}

export function StrategyBadge({ strategy }: { strategy: 'SHIELD' | 'SPEAR' | 'EXIT' }) {
  if (strategy === 'SHIELD') {
    return <Badge variant="shield">🛡️ Shield</Badge>
  }
  if (strategy === 'SPEAR') {
    return <Badge variant="spear">⚔️ Spear</Badge>
  }
  return <Badge variant="default">Exit</Badge>
}
