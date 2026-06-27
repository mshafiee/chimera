import { useEffect, useCallback, useRef } from 'react'
import { useQueryClient } from '@tanstack/react-query'

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
      const { type, data } = event.detail

      console.log('[Dashboard WebSocket] Received event:', type, data)

      // Invalidate relevant queries
      switch (type) {
        case 'risk_update':
          queryClient.invalidateQueries({ queryKey: ['risk'] })
          eventHandlerRef.current.onRiskUpdate?.(data)
          break
        case 'signal_update':
          queryClient.invalidateQueries({ queryKey: ['signals'] })
          eventHandlerRef.current.onSignalUpdate?.(data)
          break
        case 'portfolio_heat_update':
          queryClient.invalidateQueries({ queryKey: ['risk', 'portfolio'] })
          eventHandlerRef.current.onHeatAlert?.(data)
          break
        case 'consensus_alert':
          queryClient.invalidateQueries({ queryKey: ['signals', 'consensus'] })
          eventHandlerRef.current.onConsensusAlert?.(data)
          break
        case 'quality_change':
          queryClient.invalidateQueries({ queryKey: ['signals', 'quality'] })
          eventHandlerRef.current.onQualityChange?.(data)
          break
      }
    }

    // Listen for custom events from WebSocket hook
    window.addEventListener('dashboard:update' as any, handleDashboardEvent as any)

    return () => {
      window.removeEventListener('dashboard:update' as any, handleDashboardEvent as any)
    }
  }, [enabled, queryClient])

  // Manual refresh trigger
  const refreshRiskData = useCallback(() => {
    queryClient.invalidateQueries({ queryKey: ['risk'] })
  }, [queryClient])

  const refreshSignalData = useCallback(() => {
    queryClient.invalidateQueries({ queryKey: ['signals'] })
  }, [queryClient])

  const refreshAllData = useCallback(() => {
    queryClient.invalidateQueries({ queryKey: ['risk'] })
    queryClient.invalidateQueries({ queryKey: ['signals'] })
  }, [queryClient])

  return {
    refreshRiskData,
    refreshSignalData,
    refreshAllData,
  }
}