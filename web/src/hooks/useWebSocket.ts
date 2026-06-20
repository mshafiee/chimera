import { useEffect, useRef, useState, useCallback } from 'react'
import { useQueryClient } from '@tanstack/react-query'

interface WebSocketMessage {
  type: 'position_update' | 'health_update' | 'alert' | 'trade_update' | 'webhook_status' | 'webhook_health' | 'webhook_audit'
  data: unknown
}

interface UseWebSocketOptions {
  url?: string
  reconnectInterval?: number
  maxReconnectAttempts?: number
  apiKey?: string // Add API key option for WebSocket auth
}

export function useWebSocket(options: UseWebSocketOptions = {}) {
  const {
    url = `${window.location.protocol === 'https:' ? 'wss:' : 'ws:'}//${window.location.hostname}:8080/api/v1/ws`,
    reconnectInterval = 3000,
    maxReconnectAttempts = 10,
    apiKey: customApiKey = 'dev-admin-key', // Default API key for development
  } = options

  const [isConnected, setIsConnected] = useState(false)
  const [isConnecting, setIsConnecting] = useState(false)
  const [lastMessage, setLastMessage] = useState<WebSocketMessage | null>(null)
  const [connectionError, setConnectionError] = useState<string | null>(null)
  const wsRef = useRef<WebSocket | null>(null)
  const reconnectAttempts = useRef(0)
  const reconnectTimeoutRef = useRef<NodeJS.Timeout | null>(null)
  const queryClient = useQueryClient()

  const connect = useCallback(() => {
    if (wsRef.current?.readyState === WebSocket.OPEN || wsRef.current?.readyState === WebSocket.CONNECTING) {
      console.log('[WebSocket] Already connected or connecting, skipping duplicate connection')
      return
    }

    // Clear any existing reconnect timeout
    if (reconnectTimeoutRef.current) {
      clearTimeout(reconnectTimeoutRef.current)
      reconnectTimeoutRef.current = null
    }

    setIsConnecting(true)
    setConnectionError(null)

    try {
      // Use API key for WebSocket authentication instead of JWT token
      // The backend expects simple API keys for WebSocket connections
      const wsUrl = `${url}?token=${customApiKey}`
      console.log('[WebSocket] Attempting connection to:', wsUrl.replace(/token=\w+/, 'token=***'))
      const ws = new WebSocket(wsUrl)

      // Set a timeout to detect failed connections
      const connectionTimeout = setTimeout(() => {
        if (ws.readyState !== WebSocket.OPEN) {
          console.warn('[WebSocket] Connection timeout - closing')
          setConnectionError('Connection timeout - check if backend server is running')
          setIsConnecting(false)
          ws.close()
        }
      }, 5000) // 5 second timeout

      ws.onopen = () => {
        clearTimeout(connectionTimeout)
        console.log('[WebSocket] Connected successfully')
        setIsConnected(true)
        setIsConnecting(false)
        setConnectionError(null)
        reconnectAttempts.current = 0
      }

      ws.onclose = (event) => {
        clearTimeout(connectionTimeout)
        console.log('[WebSocket] Disconnected:', event.code, event.reason)
        setIsConnected(false)
        setIsConnecting(false)
        wsRef.current = null

        // Attempt reconnect if not a normal closure and not explicitly closed by client
        if (event.code !== 1000 && event.code !== 1005 && reconnectAttempts.current < maxReconnectAttempts) {
          reconnectAttempts.current++
          const backoffDelay = reconnectInterval * Math.min(reconnectAttempts.current, 5) // Exponential backoff
          console.log(
            `[WebSocket] Reconnecting in ${backoffDelay}ms (attempt ${reconnectAttempts.current}/${maxReconnectAttempts})`
          )
          setConnectionError(`Disconnected - reconnecting in ${(backoffDelay/1000).toFixed(0)}s`)

          reconnectTimeoutRef.current = setTimeout(() => {
            connect()
          }, backoffDelay)
        } else if (reconnectAttempts.current >= maxReconnectAttempts) {
          console.error('[WebSocket] Max reconnection attempts reached')
          setConnectionError('Max reconnection attempts reached - backend server may be down')
        }
      }

      ws.onerror = (error) => {
        clearTimeout(connectionTimeout)
        console.error('[WebSocket] Error:', error)
        setConnectionError('Connection error - check browser console for details')
        // Don't immediately reconnect on error, let the onclose handle it
      }

      ws.onmessage = (event) => {
        try {
          const message: WebSocketMessage = JSON.parse(event.data)
          console.log('[WebSocket] Received message:', message.type)
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
            case 'webhook_status':
            case 'webhook_health':
              queryClient.invalidateQueries({ queryKey: ['webhooks'] })
              break
            case 'webhook_audit':
              queryClient.invalidateQueries({ queryKey: ['webhooks', 'audit'] })
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
      setConnectionError('Failed to establish connection')
    }
  }, [url, reconnectInterval, maxReconnectAttempts, queryClient, customApiKey])

  const disconnect = useCallback(() => {
    // Clear any reconnect timeout
    if (reconnectTimeoutRef.current) {
      clearTimeout(reconnectTimeoutRef.current)
      reconnectTimeoutRef.current = null
    }

    if (wsRef.current) {
      wsRef.current.close(1000, 'Client disconnect')
      wsRef.current = null
      setIsConnected(false)
      setIsConnecting(false)
      setConnectionError(null)
      reconnectAttempts.current = 0
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
    isConnecting,
    connectionError,
    lastMessage,
    connect,
    disconnect,
    send,
  }
}
