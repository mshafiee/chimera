import { useState, useEffect } from 'react'
import { X, AlertTriangle, TrendingUp, Shield, Activity } from 'lucide-react'
import { clsx } from 'clsx'

interface Alert {
  id: string
  type: 'risk' | 'signal' | 'heat' | 'consensus' | 'quality'
  severity: 'low' | 'medium' | 'high'
  message: string
  timestamp: string
}

interface RealTimeAlertsProps {
  maxAlerts?: number
  className?: string
}

export function RealTimeAlerts({ maxAlerts = 5, className = '' }: RealTimeAlertsProps) {
  const [alerts, setAlerts] = useState<Alert[]>([])

  useEffect(() => {
    // Listen for dashboard update events
    const handleDashboardUpdate = (event: CustomEvent) => {
      const { type, data } = event.detail

      const newAlert: Alert = {
        id: `${Date.now()}-${Math.random()}`,
        type: type.replace('_update', '').replace('_alert', '').replace('_change', '') as Alert['type'],
        severity: data.severity || 'medium',
        message: data.message || `${type.replace('_', ' ')} detected`,
        timestamp: data.timestamp || new Date().toISOString(),
      }

      setAlerts(prev => [newAlert, ...prev].slice(0, maxAlerts))
    }

    window.addEventListener('dashboard:update', handleDashboardUpdate as any)

    return () => {
      window.removeEventListener('dashboard:update', handleDashboardUpdate as any)
    }
  }, [maxAlerts])

  const dismissAlert = (id: string) => {
    setAlerts(prev => prev.filter(alert => alert.id !== id))
  }

  const getAlertIcon = (type: Alert['type']) => {
    switch (type) {
      case 'risk':
        return Shield
      case 'signal':
        return TrendingUp
      case 'heat':
        return AlertTriangle
      case 'consensus':
        return Activity
      case 'quality':
        return TrendingUp
      default:
        return AlertTriangle
    }
  }

  const getAlertColor = (severity: Alert['severity']) => {
    switch (severity) {
      case 'high':
        return 'bg-red-50 border-red-200 text-red-800'
      case 'medium':
        return 'bg-yellow-50 border-yellow-200 text-yellow-800'
      case 'low':
        return 'bg-blue-50 border-blue-200 text-blue-800'
      default:
        return 'bg-gray-50 border-gray-200 text-gray-800'
    }
  }

  if (alerts.length === 0) {
    return null
  }

  return (
    <div className={clsx('fixed top-4 right-4 z-50 space-y-2 max-w-sm', className)}>
      {alerts.map(alert => {
        const Icon = getAlertIcon(alert.type)
        const colorClass = getAlertColor(alert.severity)

        return (
          <div
            key={alert.id}
            className={clsx(
              'p-4 rounded-lg border shadow-lg transition-all duration-300',
              colorClass
            )}
          >
            <div className="flex items-start gap-3">
              <Icon className="w-5 h-5 flex-shrink-0 mt-0.5" />
              <div className="flex-1 min-w-0">
                <p className="text-sm font-medium">{alert.message}</p>
                <p className="text-xs opacity-75 mt-1">
                  {new Date(alert.timestamp).toLocaleTimeString()}
                </p>
              </div>
              <button
                onClick={() => dismissAlert(alert.id)}
                className="flex-shrink-0 opacity-60 hover:opacity-100 transition-opacity"
              >
                <X className="w-4 h-4" />
              </button>
            </div>
          </div>
        )
      })}
    </div>
  )
}