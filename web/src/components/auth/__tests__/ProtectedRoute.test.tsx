import { describe, it, expect, beforeEach } from 'vitest'
import { render, screen } from '@testing-library/react'
import { MemoryRouter, Routes, Route } from 'react-router-dom'
import { ProtectedRoute } from '../ProtectedRoute'
import { useAuthStore } from '../../../stores/authStore'

function renderAt(path: string, children: React.ReactNode) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path={path} element={<ProtectedRoute>{children}</ProtectedRoute>} />
        <Route path="/login" element={<div>Login page</div>} />
        <Route path="/dashboard" element={<div>Dashboard page</div>} />
      </Routes>
    </MemoryRouter>
  )
}

function login(role: 'readonly' | 'operator' | 'admin') {
  useAuthStore.setState({
    user: { identifier: 'u', role, token: 'tok' },
    isAuthenticated: true,
    tokenExpiresAt: Date.now() + 3600000,
    refreshToken: null,
    lastActivity: Date.now(),
  })
}

beforeEach(() => {
  useAuthStore.setState({
    user: null,
    isAuthenticated: false,
    tokenExpiresAt: null,
    refreshToken: null,
    lastActivity: null,
  })
})

describe('ProtectedRoute', () => {
  it('redirects to login when the session has expired', () => {
    useAuthStore.setState({
      isAuthenticated: true,
      lastActivity: Date.now() - 31 * 60 * 1000,
    })
    renderAt('/protected', <div>Secret</div>)
    expect(screen.getByText('Login page')).toBeInTheDocument()
    expect(screen.queryByText('Secret')).not.toBeInTheDocument()
  })

  it('redirects to login when unauthenticated', () => {
    renderAt('/protected', <div>Secret</div>)
    expect(screen.getByText('Login page')).toBeInTheDocument()
  })

  it('redirects to dashboard when the role is insufficient', () => {
    login('readonly')
    renderAt(
      '/protected',
      <ProtectedRoute requireRole="admin">
        <div>Admin only</div>
      </ProtectedRoute>
    )
    expect(screen.getByText('Dashboard page')).toBeInTheDocument()
    expect(screen.queryByText('Admin only')).not.toBeInTheDocument()
  })

  it('renders children when authenticated with the required role', () => {
    login('admin')
    renderAt(
      '/protected',
      <ProtectedRoute requireRole="admin">
        <div>Admin only</div>
      </ProtectedRoute>
    )
    expect(screen.getByText('Admin only')).toBeInTheDocument()
  })

  it('renders children without a role requirement', () => {
    login('readonly')
    renderAt('/protected', <div>Any user</div>)
    expect(screen.getByText('Any user')).toBeInTheDocument()
  })

  it('uses the custom redirect target', () => {
    renderAt(
      '/protected',
      <ProtectedRoute redirectTo="/login">
        <div>Secret</div>
      </ProtectedRoute>
    )
    expect(screen.getByText('Login page')).toBeInTheDocument()
  })
})
