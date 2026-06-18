import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import type { AuthUser, Role } from '../types'

// Token will expire 24 hours after issuance (in milliseconds)
export const TOKEN_EXPIRY_MS = 24 * 60 * 60 * 1000
// Session timeout after 30 minutes of inactivity
export const SESSION_TIMEOUT_MS = 30 * 60 * 1000

interface AuthState {
  user: AuthUser | null
  isAuthenticated: boolean
  tokenExpiresAt: number | null
  refreshToken: string | null
  lastActivity: number | null
  _hasHydrated: boolean
  login: (user: AuthUser, expiresIn?: number) => void
  logout: () => void
  hasPermission: (required: Role) => boolean
  isTokenExpired: () => boolean
  updateTokens: (accessToken: string, refreshToken: string, expiresIn: number) => void
  updateActivity: () => void
  isSessionExpired: () => boolean
}

const roleHierarchy: Record<Role, number> = {
  readonly: 1,
  operator: 2,
  admin: 3,
}

export const useAuthStore = create<AuthState>()(
  persist(
    (set, get) => ({
      user: null,
      isAuthenticated: false,
      tokenExpiresAt: null,
      refreshToken: null,
      lastActivity: null,
      _hasHydrated: false,

      login: (user: AuthUser, expiresIn?: number) => {
        const now = Date.now()
        const expiresAt = expiresIn ? now + expiresIn * 1000 : now + TOKEN_EXPIRY_MS
        set({
          user,
          isAuthenticated: true,
          tokenExpiresAt: expiresAt,
          lastActivity: now,
        })
      },

      logout: () => {
        set({
          user: null,
          isAuthenticated: false,
          tokenExpiresAt: null,
          refreshToken: null,
          lastActivity: null,
        })
      },

      hasPermission: (required: Role) => {
        const { user } = get()
        if (!user) return false
        return roleHierarchy[user.role] >= roleHierarchy[required]
      },

      isTokenExpired: () => {
        const { tokenExpiresAt } = get()
        if (!tokenExpiresAt) return true
        // Add 5 minute buffer before actual expiration
        return Date.now() > tokenExpiresAt - 5 * 60 * 1000
      },

      updateTokens: (accessToken: string, refreshToken: string, expiresIn: number) => {
        const now = Date.now()
        const expiresAt = now + expiresIn * 1000
        const { user } = get()
        set({
          tokenExpiresAt: expiresAt,
          refreshToken,
          user: user ? { ...user, token: accessToken } : null,
        })
      },

      updateActivity: () => {
        set({ lastActivity: Date.now() })
      },

      isSessionExpired: () => {
        const { lastActivity } = get()
        if (!lastActivity) return false
        return Date.now() - lastActivity > SESSION_TIMEOUT_MS
      },
    }),
    {
      name: 'chimera-auth',
      partialize: (state) => ({
        user: state.user,
        isAuthenticated: state.isAuthenticated,
        tokenExpiresAt: state.tokenExpiresAt,
        refreshToken: state.refreshToken,
        lastActivity: state.lastActivity,
      }),
      onRehydrateStorage: () => (state) => {
        state?.setState({ _hasHydrated: true })
      },
    }
  )
)
