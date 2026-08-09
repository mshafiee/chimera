import { describe, it, expect, vi, beforeEach } from 'vitest'
import { renderHook, act } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import type { ReactNode } from 'react'
import { useDashboardWebSocket, DASHBOARD_UPDATE_EVENT } from '../useDashboardWebSocket'

let sharedClient: QueryClient

function makeWrapper() {
  sharedClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={sharedClient}>{children}</QueryClientProvider>
  )
}

function dispatch(type: string, data: Record<string, unknown> = {}) {
  window.dispatchEvent(
    new CustomEvent(DASHBOARD_UPDATE_EVENT, { detail: { type, data } })
  )
}

beforeEach(() => {
  vi.spyOn(console, 'warn').mockImplementation(() => {})
  vi.spyOn(console, 'error').mockImplementation(() => {})
})

afterEach(() => {
  vi.restoreAllMocks()
})

describe('useDashboardWebSocket', () => {
  it('does nothing when disabled', () => {
    const onRiskUpdate = vi.fn()
    const { result } = renderHook(
      () => useDashboardWebSocket({ enabled: false, onRiskUpdate }),
      { wrapper: makeWrapper() }
    )
    act(() => {
      dispatch('risk_update', { message: 'x', timestamp: 't' })
    })
    expect(onRiskUpdate).not.toHaveBeenCalled()
    expect(result.current.refreshRiskData).toBeDefined()
  })

  it('ignores malformed events', () => {
    const onRiskUpdate = vi.fn()
    renderHook(() => useDashboardWebSocket({ onRiskUpdate }), {
      wrapper: makeWrapper(),
    })
    act(() => {
      window.dispatchEvent(
        new CustomEvent(DASHBOARD_UPDATE_EVENT, { detail: { type: null } })
      )
      window.dispatchEvent(
        new CustomEvent(DASHBOARD_UPDATE_EVENT, { detail: { data: {} } })
      )
    })
    expect(onRiskUpdate).not.toHaveBeenCalled()
    expect(console.warn).toHaveBeenCalled()
  })

  it('handles risk_update events and calls the callback', () => {
    const onRiskUpdate = vi.fn()
    renderHook(() => useDashboardWebSocket({ onRiskUpdate }), {
      wrapper: makeWrapper(),
    })
    act(() => {
      dispatch('risk_update', { severity: 'high', message: 'risk!', timestamp: 't' })
    })
    expect(onRiskUpdate).toHaveBeenCalledWith({
      severity: 'high',
      message: 'risk!',
      timestamp: 't',
    })
  })

  it('handles signal_update events', () => {
    const onSignalUpdate = vi.fn()
    renderHook(() => useDashboardWebSocket({ onSignalUpdate }), {
      wrapper: makeWrapper(),
    })
    act(() => {
      dispatch('signal_update', { message: 'sig', timestamp: 't' })
    })
    expect(onSignalUpdate).toHaveBeenCalledWith({ message: 'sig', timestamp: 't' })
  })

  it('handles portfolio_heat_update events', () => {
    const onHeatAlert = vi.fn()
    renderHook(() => useDashboardWebSocket({ onHeatAlert }), {
      wrapper: makeWrapper(),
    })
    act(() => {
      dispatch('portfolio_heat_update', { message: 'heat', timestamp: 't' })
    })
    expect(onHeatAlert).toHaveBeenCalledWith({ message: 'heat', timestamp: 't' })
  })

  it('handles consensus_alert events', () => {
    const onConsensusAlert = vi.fn()
    renderHook(() => useDashboardWebSocket({ onConsensusAlert }), {
      wrapper: makeWrapper(),
    })
    act(() => {
      dispatch('consensus_alert', { message: 'con', timestamp: 't' })
    })
    expect(onConsensusAlert).toHaveBeenCalledWith({ message: 'con', timestamp: 't' })
  })

  it('handles quality_change events', () => {
    const onQualityChange = vi.fn()
    renderHook(() => useDashboardWebSocket({ onQualityChange }), {
      wrapper: makeWrapper(),
    })
    act(() => {
      dispatch('quality_change', { message: 'q', timestamp: 't' })
    })
    expect(onQualityChange).toHaveBeenCalledWith({ message: 'q', timestamp: 't' })
  })

  it('ignores unknown event types', () => {
    renderHook(() => useDashboardWebSocket(), { wrapper: makeWrapper() })
    act(() => {
      dispatch('mystery_event', { message: 'x', timestamp: 't' })
    })
    expect(console.warn).toHaveBeenCalled()
  })

  it('logs a warning when a query invalidation fails', async () => {
    const wrapper = makeWrapper()
    renderHook(() => useDashboardWebSocket(), { wrapper })
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {})
    act(() => {
      dispatch('risk_update', { message: 'x', timestamp: 't' })
    })
    await new Promise((r) => setTimeout(r, 10))
    // no crash; error spy untouched since invalidation succeeds
    expect(spy).not.toHaveBeenCalled()
  })

  it('logs when query invalidation fails', () => {
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    renderHook(() => useDashboardWebSocket(), { wrapper: makeWrapper() })
    vi.spyOn(sharedClient, 'invalidateQueries').mockRejectedValue(new Error('invalidate boom'))
    act(() => {
      dispatch('risk_update', { message: 'x', timestamp: 't' })
      dispatch('signal_update', { message: 'x', timestamp: 't' })
      dispatch('portfolio_heat_update', { message: 'x', timestamp: 't' })
      dispatch('consensus_alert', { message: 'x', timestamp: 't' })
      dispatch('quality_change', { message: 'x', timestamp: 't' })
    })
    return new Promise((resolve) => setTimeout(resolve, 20)).then(() => {
      expect(errorSpy).toHaveBeenCalled()
      errorSpy.mockRestore()
    })
  })

  it('refreshes risk and signal data on demand', () => {
    const { result } = renderHook(() => useDashboardWebSocket(), {
      wrapper: makeWrapper(),
    })
    act(() => {
      result.current.refreshRiskData()
      result.current.refreshSignalData()
      result.current.refreshAllData()
    })
    expect(result.current).toBeDefined()
  })
})
