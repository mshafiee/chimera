import { describe, it, expect, beforeEach } from 'vitest'
import { useAuthStore } from '../authStore'
import type { AuthUser } from '../../types'

const adminUser: AuthUser = { identifier: 'admin1', role: 'admin', token: 'jwt.token.abc' }
const operatorUser: AuthUser = { identifier: 'op1', role: 'operator', token: 'jwt.token.xyz' }
const readonlyUser: AuthUser = { identifier: 'ro1', role: 'readonly', token: 'jwt.token.123' }

beforeEach(() => {
  useAuthStore.setState({
    user: null,
    isAuthenticated: false,
    tokenExpiresAt: null,
    refreshToken: null,
    lastActivity: null,
  })
})

describe('authStore', () => {
  it('login stores user and sets isAuthenticated to true', () => {
    const store = useAuthStore.getState()
    store.login(adminUser)

    const state = useAuthStore.getState()
    expect(state.user).toEqual(adminUser)
    expect(state.isAuthenticated).toBe(true)
    expect(state.tokenExpiresAt).toBeGreaterThan(Date.now())
  })

  it('logout clears all auth state', () => {
    useAuthStore.getState().login(adminUser)
    useAuthStore.getState().logout()

    const state = useAuthStore.getState()
    expect(state.user).toBeNull()
    expect(state.isAuthenticated).toBe(false)
    expect(state.tokenExpiresAt).toBeNull()
    expect(state.refreshToken).toBeNull()
  })

  it('hasPermission enforces role hierarchy: admin can do everything', () => {
    const store = useAuthStore.getState()
    store.login(adminUser)

    expect(store.hasPermission('readonly')).toBe(true)
    expect(store.hasPermission('operator')).toBe(true)
    expect(store.hasPermission('admin')).toBe(true)
  })

  it('hasPermission enforces role hierarchy: operator cannot admin', () => {
    useAuthStore.getState().login(operatorUser)
    const store = useAuthStore.getState()

    expect(store.hasPermission('readonly')).toBe(true)
    expect(store.hasPermission('operator')).toBe(true)
    expect(store.hasPermission('admin')).toBe(false)
  })

  it('hasPermission enforces role hierarchy: readonly cannot operator', () => {
    useAuthStore.getState().login(readonlyUser)
    const store = useAuthStore.getState()

    expect(store.hasPermission('readonly')).toBe(true)
    expect(store.hasPermission('operator')).toBe(false)
    expect(store.hasPermission('admin')).toBe(false)
  })

  it('hasPermission returns false when no user is logged in', () => {
    expect(useAuthStore.getState().hasPermission('readonly')).toBe(false)
  })

  it('isTokenExpired returns false within expiry', () => {
    const future = Date.now() + 3600000
    useAuthStore.setState({ tokenExpiresAt: future, isAuthenticated: true })
    expect(useAuthStore.getState().isTokenExpired()).toBe(false)
  })

  it('isTokenExpired returns true when expired', () => {
    const past = Date.now() - 3600000
    useAuthStore.setState({ tokenExpiresAt: past, isAuthenticated: true })
    expect(useAuthStore.getState().isTokenExpired()).toBe(true)
  })

  it('isTokenExpired applies 5-minute buffer before actual expiry', () => {
    const almostExpired = Date.now() + 120000
    useAuthStore.setState({ tokenExpiresAt: almostExpired, isAuthenticated: true })
    expect(useAuthStore.getState().isTokenExpired()).toBe(true)
  })

  it('isSessionExpired returns true after 30 minutes of inactivity', () => {
    const longAgo = Date.now() - 31 * 60 * 1000
    useAuthStore.setState({ lastActivity: longAgo, isAuthenticated: true })
    expect(useAuthStore.getState().isSessionExpired()).toBe(true)
  })

  it('isSessionExpired returns false within 30 minutes', () => {
    const recent = Date.now() - 15 * 60 * 1000
    useAuthStore.setState({ lastActivity: recent, isAuthenticated: true })
    expect(useAuthStore.getState().isSessionExpired()).toBe(false)
  })
})
