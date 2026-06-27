import { Wifi, WifiOff, Loader2 } from 'lucide-react'
import { clsx } from 'clsx'

interface ConnectionStatusProps {
  isConnected: boolean
  isConnecting: boolean
  connectionError: string | null
  className?: string
}

export function ConnectionStatus({
  isConnected,
  isConnecting,
  connectionError,
  className = ''
}: ConnectionStatusProps) {
  const getStatusText = () => {
    if (isConnecting) return 'Connecting...'
    if (connectionError) return 'Connection Error'
    if (isConnected) return 'Live'
    return 'Disconnected'
  }

  const getStatusColor = () => {
    if (isConnecting) return 'text-yellow-600 bg-yellow-50 border-yellow-200'
    if (connectionError) return 'text-red-600 bg-red-50 border-red-200'
    if (isConnected) return 'text-green-600 bg-green-50 border-green-200'
    return 'text-gray-600 bg-gray-50 border-gray-200'
  }

  const getIcon = () => {
    if (isConnecting) return Loader2
    if (connectionError || !isConnected) return WifiOff
    return Wifi
  }

  const Icon = getIcon()
  const colorClass = getStatusColor()

  return (
    <div
      className={clsx(
        'flex items-center gap-2 px-3 py-1.5 rounded-full border text-xs font-medium',
        colorClass,
        className
      )}
    >
      <Icon className={clsx('w-3.5 h-3.5', isConnecting && 'animate-spin')} />
      <span>{getStatusText()}</span>
      {isConnected && (
        <span className="w-2 h-2 bg-green-500 rounded-full animate-pulse" />
      )}
    </div>
  )
}