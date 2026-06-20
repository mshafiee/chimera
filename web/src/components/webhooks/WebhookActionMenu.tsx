import { useState } from 'react'
import { Button } from '../ui/Button'
import { Badge } from '../ui/Badge'
import { MoreVertical, RefreshCw, Power, PowerOff } from 'lucide-react'

interface WebhookActionMenuProps {
  walletAddress: string
  isEnabled?: boolean
  onToggle: (walletAddress: string, enabled: boolean) => Promise<void>
  onRetry: (walletAddress: string) => Promise<void>
  isLoading?: boolean
}

export function WebhookActionMenu({
  walletAddress,
  isEnabled = true,
  onToggle,
  onRetry,
  isLoading = false,
}: WebhookActionMenuProps) {
  const [isOpen, setIsOpen] = useState(false)

  const handleToggle = async () => {
    setIsOpen(false)
    await onToggle(walletAddress, !isEnabled)
  }

  const handleRetry = async () => {
    setIsOpen(false)
    await onRetry(walletAddress)
  }

  return (
    <div className="relative">
      <Button
        variant="ghost"
        size="sm"
        onClick={() => setIsOpen(!isOpen)}
        disabled={isLoading}
        className="p-1"
      >
        <MoreVertical className="w-4 h-4" />
      </Button>

      {isOpen && (
        <>
          <div
            className="fixed inset-0 z-10"
            onClick={() => setIsOpen(false)}
          />
          <div className="absolute right-0 top-8 z-20 w-48 bg-surface border border-border rounded-lg shadow-lg py-1">
            <button
              onClick={handleToggle}
              disabled={isLoading}
              className="w-full px-4 py-2 text-left text-sm hover:bg-surface-light flex items-center gap-3 disabled:opacity-50"
            >
              {isEnabled ? (
                <>
                  <PowerOff className="w-4 h-4 text-spear" />
                  <span>Disable Webhook</span>
                </>
              ) : (
                <>
                  <Power className="w-4 h-4 text-profit" />
                  <span>Enable Webhook</span>
                </>
              )}
            </button>

            <button
              onClick={handleRetry}
              disabled={isLoading}
              className="w-full px-4 py-2 text-left text-sm hover:bg-surface-light flex items-center gap-3 disabled:opacity-50"
            >
              <RefreshCw className="w-4 h-4 text-shield" />
              <span>Retry Registration</span>
            </button>
          </div>
        </>
      )}
    </div>
  )
}

// Simple status badge for webhook state
interface WebhookStatusBadgeProps {
  status: 'active' | 'paused' | 'failed' | 'orphaned'
  healthStatus?: 'healthy' | 'unhealthy' | 'unknown' | 'timeout' | 'error'
}

export function WebhookStatusBadge({ status, healthStatus }: WebhookStatusBadgeProps) {
  const getStatusVariant = (): 'success' | 'warning' | 'danger' | 'default' => {
    if (status === 'active') {
      if (healthStatus === 'healthy') return 'success'
      if (healthStatus === 'unhealthy' || healthStatus === 'error') return 'danger'
      return 'warning'
    }
    if (status === 'paused') return 'default'
    return 'danger'
  }

  return (
    <Badge variant={getStatusVariant()} size="sm">
      {status}
    </Badge>
  )
}
