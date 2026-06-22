import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useAuthStore } from '../../stores/authStore'
import type { AuthUser } from '../../types'

vi.mock('axios', () => {
  const mockAxiosInstance = {
    get: vi.fn(),
    post: vi.fn(),
    interceptors: { request: { use: vi.fn() }, response: { use: vi.fn() } },
  }
  return {
    default: {
      create: vi.fn(() => mockAxiosInstance),
    },
  }
})

const adminUser: AuthUser = { identifier: 'admin1', role: 'admin', token: 'jwt.token.abc' }

describe('authStore login/logout logic', () => {
  beforeEach(() => {
    useAuthStore.setState({
      user: null,
      isAuthenticated: false,
      tokenExpiresAt: null,
      refreshToken: null,
      lastActivity: null,
    })
  })

  it('login with user stores identifier, role, and token', () => {
    useAuthStore.getState().login(adminUser)
    const state = useAuthStore.getState()
    expect(state.user?.identifier).toBe('admin1')
    expect(state.user?.role).toBe('admin')
    expect(state.user?.token).toBe('jwt.token.abc')
  })

  it('login sets tokenExpiresAt to future timestamp', () => {
    useAuthStore.getState().login(adminUser)
    expect(useAuthStore.getState().tokenExpiresAt).toBeGreaterThan(Date.now())
  })

  it('logout clears user, isAuthenticated, and refreshToken', () => {
    useAuthStore.getState().login(adminUser)
    useAuthStore.setState({ refreshToken: 'refresh-token' })
    useAuthStore.getState().logout()

    const state = useAuthStore.getState()
    expect(state.user).toBeNull()
    expect(state.isAuthenticated).toBe(false)
    expect(state.refreshToken).toBeNull()
  })

  it('updateTokens updates access token and refresh token', () => {
    useAuthStore.getState().login(adminUser)
    useAuthStore.getState().updateTokens('new.jwt.token', 'new-refresh', 86400)

    const state = useAuthStore.getState()
    expect(state.user?.token).toBe('new.jwt.token')
    expect(state.refreshToken).toBe('new-refresh')
    expect(state.tokenExpiresAt).toBeGreaterThan(Date.now())
  })

  it('updateActivity updates lastActivity to recent time', () => {
    const oldActivity = Date.now() - 60000
    useAuthStore.setState({ lastActivity: oldActivity, isAuthenticated: true })
    useAuthStore.getState().updateActivity()
    expect(useAuthStore.getState().lastActivity).toBeGreaterThan(oldActivity)
  })
})
