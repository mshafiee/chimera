import { useEffect, useCallback, useRef } from 'react'
import { useQueryClient } from '@tanstack/react-query'

export const DASHBOARD_UPDATE_EVENT = 'dashboard:update'

interface DashboardWebSocketMessage {
  type: 'risk_update' | 'signal_update' | 'portfolio_heat_update' | 'consensus_alert' | 'quality_change'
  data: {
    severity?: 'low' | 'medium' | 'high'
    timestamp: string
    message: string
    metrics?: Record<string, unknown>
  }
}

interface UseDashboardWebSocketOptions {
  enabled?: boolean
  onRiskUpdate?: (data: DashboardWebSocketMessage['data']) => void
  onSignalUpdate?: (data: DashboardWebSocketMessage['data']) => void
  onHeatAlert?: (data: DashboardWebSocketMessage['data']) => void
  onConsensusAlert?: (data: DashboardWebSocketMessage['data']) => void
  onQualityChange?: (data: DashboardWebSocketMessage['data']) => void
}

export function useDashboardWebSocket({
  enabled = true,
  onRiskUpdate,
  onSignalUpdate,
  onHeatAlert,
  onConsensusAlert,
  onQualityChange,
}: UseDashboardWebSocketOptions = {}) {
  const queryClient = useQueryClient()
  const eventHandlerRef = useRef<{
    onRiskUpdate?: (data: DashboardWebSocketMessage['data']) => void
    onSignalUpdate?: (data: DashboardWebSocketMessage['data']) => void
    onHeatAlert?: (data: DashboardWebSocketMessage['data']) => void
    onConsensusAlert?: (data: DashboardWebSocketMessage['data']) => void
    onQualityChange?: (data: DashboardWebSocketMessage['data']) => void
  }>({})

  // Update ref when callbacks change
  useEffect(() => {
    eventHandlerRef.current = {
      onRiskUpdate,
      onSignalUpdate,
      onHeatAlert,
      onConsensusAlert,
      onQualityChange,
    }
  }, [onRiskUpdate, onSignalUpdate, onHeatAlert, onConsensusAlert, onQualityChange])

  // Handle custom dashboard events
  useEffect(() => {
    if (!enabled) return

    const handleDashboardEvent = (event: CustomEvent<DashboardWebSocketMessage>) => {
      const detail = event.detail
      const type = detail?.type
      const data = detail?.data
      if (!type || !data) {
        console.warn('[Dashboard WebSocket] Ignoring malformed event', event.detail)
        return
      }

      if (import.meta.env.DEV) {
        console.log('[Dashboard WebSocket] Received event:', type, data)
      }

      // Invalidate relevant queries
      switch (type) {
        case 'risk_update':
          void queryClient.invalidateQueries({ queryKey: ['risk'] }).catch(() => {
            console.error('[Dashboard WebSocket] Failed to refresh risk data')
          })
          eventHandlerRef.current.onRiskUpdate?.(data)
          break
        case 'signal_update':
          void queryClient.invalidateQueries({ queryKey: ['signals'] }).catch(() => {
            console.error('[Dashboard WebSocket] Failed to refresh signal data')
          })
          eventHandlerRef.current.onSignalUpdate?.(data)
          break
        case 'portfolio_heat_update':
          void queryClient.invalidateQueries({ queryKey: ['risk', 'portfolio'] }).catch(() => {
            console.error('[Dashboard WebSocket] Failed to refresh portfolio heat data')
          })
          eventHandlerRef.current.onHeatAlert?.(data)
          break
        case 'consensus_alert':
          void queryClient.invalidateQueries({ queryKey: ['signals', 'consensus'] }).catch(() => {
            console.error('[Dashboard WebSocket] Failed to refresh consensus data')
          })
          eventHandlerRef.current.onConsensusAlert?.(data)
          break
        case 'quality_change':
          void queryClient.invalidateQueries({ queryKey: ['signals', 'quality'] }).catch(() => {
            console.error('[Dashboard WebSocket] Failed to refresh quality data')
          })
          eventHandlerRef.current.onQualityChange?.(data)
          break
        default:
          console.warn('[Dashboard WebSocket] Ignoring unknown event type:', type)
      }
    }

    // Listen for custom events from WebSocket hook
    window.addEventListener(DASHBOARD_UPDATE_EVENT, handleDashboardEvent as EventListener)

    return () => {
      window.removeEventListener(DASHBOARD_UPDATE_EVENT, handleDashboardEvent as EventListener)
    }
  }, [enabled, queryClient])

  // Manual refresh trigger
  const refreshRiskData = useCallback(() => {
    void queryClient.invalidateQueries({ queryKey: ['risk'] })
  }, [queryClient])

  const refreshSignalData = useCallback(() => {
    void queryClient.invalidateQueries({ queryKey: ['signals'] })
  }, [queryClient])

  const refreshAllData = useCallback(() => {
    void queryClient.invalidateQueries({ queryKey: ['risk'] })
    void queryClient.invalidateQueries({ queryKey: ['signals'] })
    void queryClient.invalidateQueries({ queryKey: ['risk', 'portfolio'] })
    void queryClient.invalidateQueries({ queryKey: ['signals', 'consensus'] })
    void queryClient.invalidateQueries({ queryKey: ['signals', 'quality'] })
  }, [queryClient])

  return {
    refreshRiskData,
    refreshSignalData,
    refreshAllData,
  }
}
