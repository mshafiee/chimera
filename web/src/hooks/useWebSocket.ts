import { useEffect, useRef, useState, useCallback } from 'react'
import { useQueryClient } from '@tanstack/react-query'

interface WebSocketMessage {
  type: 'position_update' | 'health_update' | 'alert' | 'trade_update'
  data: unknown
}

interface UseWebSocketOptions {
  url?: string
  reconnectInterval?: number
  maxReconnectAttempts?: number
}

export function useWebSocket(options: UseWebSocketOptions = {}) {
  const {
    url = `${window.location.protocol === 'https:' ? 'wss:' : 'ws:'}//${window.location.host}/ws`,
    reconnectInterval = 3000,
    maxReconnectAttempts = 10,
  } = options

  const [isConnected, setIsConnected] = useState(false)
  const [lastMessage, setLastMessage] = useState<WebSocketMessage | null>(null)
  const wsRef = useRef<WebSocket | null>(null)
  const reconnectAttempts = useRef(0)
  const queryClient = useQueryClient()

  const connect = useCallback(() => {
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      return
    }

    try {
      const ws = new WebSocket(url)

      ws.onopen = () => {
        console.log('[WebSocket] Connected')
        setIsConnected(true)
        reconnectAttempts.current = 0
      }

      ws.onclose = (event) => {
        console.log('[WebSocket] Disconnected:', event.code, event.reason)
        setIsConnected(false)
        wsRef.current = null

        // Attempt reconnect if not a normal closure
        if (event.code !== 1000 && reconnectAttempts.current < maxReconnectAttempts) {
          reconnectAttempts.current++
          console.log(
            `[WebSocket] Reconnecting in ${reconnectInterval}ms (attempt ${reconnectAttempts.current})`
          )
          setTimeout(connect, reconnectInterval)
        }
      }

      ws.onerror = (error) => {
        console.error('[WebSocket] Error:', error)
      }

      ws.onmessage = (event) => {
        try {
          const message: WebSocketMessage = JSON.parse(event.data)
          setLastMessage(message)

          // Invalidate relevant queries based on message type
          switch (message.type) {
            case 'position_update':
              queryClient.invalidateQueries({ queryKey: ['positions'] })
              break
            case 'health_update':
              queryClient.invalidateQueries({ queryKey: ['health'] })
              break
            case 'trade_update':
              queryClient.invalidateQueries({ queryKey: ['trades'] })
              queryClient.invalidateQueries({ queryKey: ['positions'] })
              break
            case 'alert':
              // Could trigger a notification here
              console.log('[WebSocket] Alert:', message.data)
              break
          }
        } catch (e) {
          console.error('[WebSocket] Failed to parse message:', e)
        }
      }

      wsRef.current = ws
    } catch (error) {
      console.error('[WebSocket] Failed to connect:', error)
    }
  }, [url, reconnectInterval, maxReconnectAttempts, queryClient])

  const disconnect = useCallback(() => {
    if (wsRef.current) {
      wsRef.current.close(1000, 'Client disconnect')
      wsRef.current = null
      setIsConnected(false)
    }
  }, [])

  const send = useCallback((message: object) => {
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify(message))
    } else {
      console.warn('[WebSocket] Cannot send - not connected')
    }
  }, [])

  // Connect on mount
  useEffect(() => {
    connect()
    return () => {
      disconnect()
    }
  }, [connect, disconnect])

  return {
    isConnected,
    lastMessage,
    connect,
    disconnect,
    send,
  }
}
