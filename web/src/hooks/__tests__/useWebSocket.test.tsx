import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { renderHook, act } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import type { ReactNode } from 'react'
import { useWebSocket } from '../useWebSocket'

class FakeWebSocket {
  static instances: FakeWebSocket[] = []
  static CONNECTING = 0
  static OPEN = 1
  static CLOSING = 2
  static CLOSED = 3

  url = ''
  readyState = FakeWebSocket.CONNECTING
  onopen: (() => void) | null = null
  onclose: ((event: { code: number; reason: string }) => void) | null = null
  onerror: ((event: Event) => void) | null = null
  onmessage: ((event: { data: string }) => void) | null = null
  sent: string[] = []
  closedCode: number | null = null
  closeCalled = false

  constructor(url: string) {
    this.url = url
    FakeWebSocket.instances.push(this)
  }

  send(data: string) {
    this.sent.push(data)
  }

  close(code?: number, reason?: string) {
    this.closeCalled = true
    this.closedCode = code ?? null
    this.readyState = FakeWebSocket.CLOSED
    this.onclose?.({ code: code ?? 1000, reason: reason ?? '' })
  }
}

vi.stubGlobal('WebSocket', FakeWebSocket)

function makeWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  )
}

const wrapper = makeWrapper()

beforeEach(() => {
  vi.clearAllMocks()
  vi.useFakeTimers()
  FakeWebSocket.instances = []
})

afterEach(() => {
  vi.useRealTimers()
})

function latestInstance(): FakeWebSocket {
  return FakeWebSocket.instances[FakeWebSocket.instances.length - 1]
}

function openSocket(ws: FakeWebSocket) {
  ws.readyState = FakeWebSocket.OPEN
  ws.onopen?.()
}

describe('useWebSocket', () => {
  it('skips connecting when no API key is available', () => {
    const { result } = renderHook(() => useWebSocket({ apiKey: '' }), { wrapper })
    expect(FakeWebSocket.instances).toHaveLength(0)
    expect(result.current.isConnected).toBe(false)
  })

  it('connects with the api key in the URL and reports connected state', () => {
    const { result } = renderHook(() => useWebSocket({ apiKey: 'secret-key' }), { wrapper })
    const ws = latestInstance()
    expect(ws.url).toContain('token=secret-key')
    expect(result.current.isConnecting).toBe(true)

    act(() => {
      openSocket(ws)
    })
    expect(result.current.isConnected).toBe(true)
    expect(result.current.isConnecting).toBe(false)
    expect(result.current.connectionError).toBeNull()
  })

  it('reports a connection timeout when the socket never opens', () => {
    const { result } = renderHook(() => useWebSocket({ apiKey: 'k' }), { wrapper })
    const ws = latestInstance()
    act(() => {
      vi.advanceTimersByTime(5000)
    })
    // the timeout closes the socket, which then schedules a reconnect
    expect(ws.closeCalled).toBe(true)
    expect(result.current.isConnecting).toBe(false)
    expect(result.current.connectionError).toContain('reconnecting')
  })

  it('skips duplicate connections when already open', () => {
    const { result } = renderHook(() => useWebSocket({ apiKey: 'k' }), { wrapper })
    const ws = latestInstance()
    act(() => {
      openSocket(ws)
      result.current.connect()
    })
    expect(FakeWebSocket.instances).toHaveLength(1)
  })

  it('parses incoming messages and invalidates related queries', () => {
    const { result } = renderHook(() => useWebSocket({ apiKey: 'k' }), { wrapper })
    const ws = latestInstance()
    act(() => {
      openSocket(ws)
    })

    const types = [
      'position_update',
      'health_update',
      'trade_update',
      'risk_update',
      'signal_update',
      'portfolio_heat_update',
      'consensus_alert',
      'quality_change',
      'webhook_status',
      'webhook_health',
      'webhook_audit',
      'alert',
    ]
    types.forEach((type, i) => {
      act(() => {
        ws.onmessage?.({ data: JSON.stringify({ type, data: { message: `msg ${i}` } }) })
      })
    })
    expect(result.current.lastMessage?.type).toBe('alert')
    expect(result.current.lastMessage?.data).toEqual({ message: 'msg 11' })
  })

  it('logs and ignores malformed messages', () => {
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    renderHook(() => useWebSocket({ apiKey: 'k' }), { wrapper })
    const ws = latestInstance()
    act(() => {
      ws.onmessage?.({ data: 'not json' })
    })
    expect(errorSpy).toHaveBeenCalled()
    errorSpy.mockRestore()
  })

  it('reports errors and does not reconnect when the server closes cleanly', () => {
    const { result } = renderHook(() => useWebSocket({ apiKey: 'k' }), { wrapper })
    const ws = latestInstance()
    act(() => {
      openSocket(ws)
      ws.onerror?.(new Event('error'))
    })
    expect(result.current.connectionError).toBe('Connection error')

    act(() => {
      ws.onclose?.({ code: 1000, reason: 'clean' })
    })
    expect(result.current.isConnected).toBe(false)
    expect(FakeWebSocket.instances).toHaveLength(1)
  })

  it('reconnects with backoff after an unexpected close', () => {
    const { result } = renderHook(() => useWebSocket({ apiKey: 'k' }), { wrapper })
    const ws = latestInstance()
    act(() => {
      openSocket(ws)
      ws.onclose?.({ code: 1006, reason: 'abnormal' })
    })
    expect(result.current.connectionError).toContain('reconnecting')

    act(() => {
      vi.advanceTimersByTime(3000)
    })
    expect(FakeWebSocket.instances.length).toBeGreaterThan(1)
    expect(result.current.isConnecting).toBe(true)
  })

  it('a manual connect restarts the reconnect attempt budget', () => {
    const { result } = renderHook(() => useWebSocket({ apiKey: 'k', maxReconnectAttempts: 1 }), {
      wrapper,
    })
    const ws = latestInstance()
    act(() => {
      openSocket(ws)
      ws.onclose?.({ code: 1006, reason: 'abnormal' })
    })
    act(() => {
      result.current.connect(true)
    })
    expect(FakeWebSocket.instances.length).toBe(2)
  })

  it('reports a failure when the WebSocket constructor throws', () => {
    class ThrowingWebSocket {
      static OPEN = 1
      static CONNECTING = 0
      readyState = 0
      constructor() {
        throw new Error('no websocket support')
      }
      send() {}
      close() {}
    }
    vi.stubGlobal('WebSocket', ThrowingWebSocket)
    FakeWebSocket.instances = []
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    const { result } = renderHook(() => useWebSocket({ apiKey: 'k' }), { wrapper })
    expect(result.current.connectionError).toBe('Failed to establish connection')
    expect(errorSpy).toHaveBeenCalled()
    errorSpy.mockRestore()
    vi.stubGlobal('WebSocket', FakeWebSocket)
  })

  it('stops reconnecting after max attempts', () => {
    const { result } = renderHook(
      () => useWebSocket({ apiKey: 'k', maxReconnectAttempts: 0 }),
      { wrapper }
    )
    const ws = latestInstance()
    act(() => {
      openSocket(ws)
      ws.onclose?.({ code: 1006, reason: 'abnormal' })
    })
    expect(result.current.connectionError).toBe(
      'Max reconnection attempts reached - backend server may be down'
    )
    expect(FakeWebSocket.instances).toHaveLength(1)
  })

  it('disconnect closes the socket and prevents reconnection', () => {
    const { result } = renderHook(() => useWebSocket({ apiKey: 'k' }), { wrapper })
    const ws = latestInstance()
    act(() => {
      openSocket(ws)
      result.current.disconnect()
    })
    expect(ws.closeCalled).toBe(true)
    expect(ws.closedCode).toBe(1000)
    expect(result.current.isConnected).toBe(false)
    expect(result.current.isConnecting).toBe(false)
  })

  it('send serializes messages when open and warns otherwise', () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const { result } = renderHook(() => useWebSocket({ apiKey: 'k' }), { wrapper })
    const ws = latestInstance()

    act(() => {
      result.current.send({ hello: 'world' })
    })
    expect(warnSpy).toHaveBeenCalled()

    act(() => {
      openSocket(ws)
      result.current.send({ hello: 'world' })
    })
    expect(ws.sent).toEqual([JSON.stringify({ hello: 'world' })])
    warnSpy.mockRestore()
  })

  it('cleans up on unmount', () => {
    const { unmount } = renderHook(() => useWebSocket({ apiKey: 'k' }), { wrapper })
    const ws = latestInstance()
    unmount()
    expect(ws.closeCalled).toBe(true)
  })
})
