import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import type { AuthUser, Role } from '../types'

interface AuthState {
  user: AuthUser | null
  isAuthenticated: boolean
  login: (user: AuthUser) => void
  logout: () => void
  hasPermission: (required: Role) => boolean
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

      login: (user: AuthUser) => {
        set({ user, isAuthenticated: true })
      },

      logout: () => {
        set({ user: null, isAuthenticated: false })
      },

      hasPermission: (required: Role) => {
        const { user } = get()
        if (!user) return false
        return roleHierarchy[user.role] >= roleHierarchy[required]
      },
    }),
    {
      name: 'chimera-auth',
      partialize: (state) => ({ user: state.user, isAuthenticated: state.isAuthenticated }),
    }
  )
)
