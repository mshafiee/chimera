import { useLocation } from 'react-router-dom'
import { RefreshCw, Wifi, WifiOff } from 'lucide-react'
import { Button } from '../ui/Button'
import { ConnectWalletButton } from '../wallet'
import { useAuthStore } from '../../stores/authStore'

const pageTitles: Record<string, string> = {
  '/dashboard': 'Command Center',
  '/wallets': 'Wallet Roster',
  '/trades': 'Trade Ledger',
  '/config': 'Configuration',
  '/incidents': 'Incident Log',
}

interface HeaderProps {
  isConnected?: boolean
  lastUpdate?: Date | null
  onRefresh?: () => void
}

export function Header({ isConnected = false, lastUpdate, onRefresh }: HeaderProps) {
  const location = useLocation()
  const { user, isAuthenticated } = useAuthStore()

  const title = pageTitles[location.pathname] || 'Chimera'

  return (
    <header className="h-16 bg-surface border-b border-border flex items-center justify-between px-6">
      {/* Left: Page Title */}
      <div className="flex items-center gap-4">
        <h1 className="text-xl font-semibold text-text">{title}</h1>
      </div>

      {/* Right: Status & Actions */}
      <div className="flex items-center gap-4">
        {/* Connection Status */}
        <div className="flex items-center gap-2 text-sm">
          {isConnected ? (
            <>
              <Wifi className="w-4 h-4 text-profit" />
              <span className="text-text-muted">Live</span>
            </>
          ) : (
            <>
              <WifiOff className="w-4 h-4 text-loss" />
              <span className="text-text-muted">Disconnected</span>
            </>
          )}
        </div>

        {/* Last Update */}
        {lastUpdate && (
          <div className="text-sm text-text-muted">
            Updated {formatTimeAgo(lastUpdate)}
          </div>
        )}

        {/* Refresh Button */}
        {onRefresh && (
          <Button variant="ghost" size="sm" onClick={onRefresh}>
            <RefreshCw className="w-4 h-4" />
          </Button>
        )}

        {/* User Info */}
        {isAuthenticated && user && (
          <div className="flex items-center gap-2 pl-4 border-l border-border">
            <div className="w-8 h-8 rounded-full bg-shield/20 flex items-center justify-center">
              <span className="text-shield text-sm font-medium">
                {user.identifier.slice(0, 2).toUpperCase()}
              </span>
            </div>
            <div className="text-sm">
              <div className="text-text font-medium truncate max-w-[100px]">
                {user.identifier.slice(0, 8)}...
              </div>
              <div className="text-text-muted text-xs capitalize">{user.role}</div>
            </div>
          </div>
        )}

        {/* Connect Wallet Button */}
        <ConnectWalletButton />
      </div>
    </header>
  )
}

function formatTimeAgo(date: Date): string {
  const seconds = Math.floor((Date.now() - date.getTime()) / 1000)

  if (seconds < 5) return 'just now'
  if (seconds < 60) return `${seconds}s ago`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`
  return `${Math.floor(seconds / 86400)}d ago`
}
