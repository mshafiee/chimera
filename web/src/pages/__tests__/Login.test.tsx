import { describe, it, expect, beforeEach } from 'vitest'
import { render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { Login } from '../Login'
import { useAuthStore } from '../../stores/authStore'

vi.mock('../../components/wallet', () => ({
  ConnectWalletButton: () => <button>Connect Wallet</button>,
}))

function resetAuth() {
  useAuthStore.setState({
    user: null,
    isAuthenticated: false,
    tokenExpiresAt: null,
    refreshToken: null,
    lastActivity: null,
  })
}

describe('Login', () => {
  beforeEach(resetAuth)

  it('renders the login card when unauthenticated', () => {
    render(
      <MemoryRouter>
        <Login />
      </MemoryRouter>
    )
    expect(screen.getByText('Chimera')).toBeInTheDocument()
    expect(screen.getByText('High-Frequency Copy-Trading Platform')).toBeInTheDocument()
    expect(screen.getByText('Connect Your Wallet')).toBeInTheDocument()
    expect(screen.getByText(/Chimera v/)).toBeInTheDocument()
    expect(screen.getByText(/No transactions will be executed/)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Connect Wallet' })).toBeInTheDocument()
  })

  it('redirects to the dashboard when already authenticated', () => {
    useAuthStore.setState({
      user: { identifier: 'admin', role: 'admin', token: 'tok' },
      isAuthenticated: true,
      tokenExpiresAt: Date.now() + 3600000,
      refreshToken: null,
      lastActivity: Date.now(),
    })
    render(
      <MemoryRouter initialEntries={['/login']}>
        <Login />
      </MemoryRouter>
    )
    expect(screen.queryByText('Connect Your Wallet')).not.toBeInTheDocument()
  })
})
