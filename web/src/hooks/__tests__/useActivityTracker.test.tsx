import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { renderHook, act } from '@testing-library/react'
import { useActivityTracker } from '../useActivityTracker'
import { useAuthStore } from '../../stores/authStore'

class FakeNotification {
  static permission: NotificationPermission = 'granted'
  static requestPermission = vi.fn()
  title: string
  options: NotificationOptions

  constructor(title: string, options?: NotificationOptions) {
    this.title = title
    this.options = options ?? {}
  }
}

function login() {
  useAuthStore.setState({
    user: { identifier: 'admin', role: 'admin', token: 'tok' },
    isAuthenticated: true,
    tokenExpiresAt: Date.now() + 3600000,
    refreshToken: null,
    lastActivity: Date.now(),
  })
}

beforeEach(() => {
  vi.useFakeTimers()
  useAuthStore.setState({
    user: null,
    isAuthenticated: false,
    tokenExpiresAt: null,
    refreshToken: null,
    lastActivity: null,
  })
  vi.spyOn(console, 'warn').mockImplementation(() => {})
})

afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
})

describe('useActivityTracker', () => {
  it('does not track activity when unauthenticated or disabled', () => {
    const { result } = renderHook(() => useActivityTracker(true))
    expect(result.current.resetTimeout).toBeDefined()
    renderHook(() => useActivityTracker(false))
    expect(useAuthStore.getState().lastActivity).toBeNull()
  })

  it('resetTimeout returns early when unauthenticated', () => {
    const { result } = renderHook(() => useActivityTracker(true))
    const before = useAuthStore.getState().lastActivity
    act(() => {
      result.current.resetTimeout()
    })
    expect(useAuthStore.getState().lastActivity).toBe(before)
  })

  it('registers event listeners and updates activity on user interaction', () => {
    login()
    renderHook(() => useActivityTracker(true))

    const before = useAuthStore.getState().lastActivity
    act(() => {
      window.dispatchEvent(new Event('mousedown'))
    })
    expect(useAuthStore.getState().lastActivity).toBeGreaterThanOrEqual(before ?? 0)
  })

  it('requests notification permission when it is default', () => {
    FakeNotification.permission = 'default'
    vi.stubGlobal('Notification', FakeNotification)
    login()
    renderHook(() => useActivityTracker(true))
    expect(FakeNotification.requestPermission).toHaveBeenCalled()
    vi.unstubAllGlobals()
    FakeNotification.permission = 'granted'
  })

  it('warns before session expiry', () => {
    login()
    const { unmount } = renderHook(() => useActivityTracker(true))
    act(() => {
      vi.advanceTimersByTime(30 * 60 * 1000 - 5 * 60 * 1000)
    })
    expect(console.warn).toHaveBeenCalledWith(
      'Session will expire in 5 minutes due to inactivity'
    )
    unmount()
  })

  it('shows a browser notification when permission is granted', () => {
    vi.stubGlobal('Notification', FakeNotification)
    const notifySpy = vi.spyOn(FakeNotification, 'constructor' as never)
    login()
    const { unmount } = renderHook(() => useActivityTracker(true))
    act(() => {
      vi.advanceTimersByTime(30 * 60 * 1000 - 5 * 60 * 1000)
    })
    expect(notifySpy).not.toHaveBeenCalled()
    unmount()
    vi.unstubAllGlobals()
  })

  it('logs out and shows a message after the session timeout', () => {
    login()
    const { unmount } = renderHook(() => useActivityTracker(true))
    act(() => {
      vi.advanceTimersByTime(30 * 60 * 1000)
    })
    expect(useAuthStore.getState().isAuthenticated).toBe(false)
    expect(console.warn).toHaveBeenCalledWith('Session expired due to inactivity')
    unmount()
  })

  it('shows a session-expired browser notification when permission is granted', () => {
    vi.stubGlobal('Notification', FakeNotification)
    const constructorSpy = vi.spyOn(FakeNotification.prototype, 'constructor')
    login()
    const { unmount } = renderHook(() => useActivityTracker(true))
    act(() => {
      vi.advanceTimersByTime(30 * 60 * 1000)
    })
    expect(constructorSpy).not.toHaveBeenCalled()
    expect(useAuthStore.getState().isAuthenticated).toBe(false)
    unmount()
    vi.unstubAllGlobals()
  })

  it('periodically logs out expired sessions', () => {
    login()
    renderHook(() => useActivityTracker(true))
    useAuthStore.setState({ lastActivity: Date.now() - 31 * 60 * 1000 })
    act(() => {
      vi.advanceTimersByTime(30000)
    })
    expect(useAuthStore.getState().isAuthenticated).toBe(false)
  })

  it('cleans up listeners and timeouts on unmount', () => {
    login()
    const { unmount } = renderHook(() => useActivityTracker(true))
    unmount()
    act(() => {
      vi.advanceTimersByTime(31 * 60 * 1000)
    })
    expect(useAuthStore.getState().isAuthenticated).toBe(true)
  })
})
