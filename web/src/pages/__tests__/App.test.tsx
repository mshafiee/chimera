import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, Outlet } from 'react-router-dom'
import App from '../../App'
import { useAuthStore } from '../../stores/authStore'

vi.mock('../../components/layout/Layout', () => ({
  Layout: () => (
    <div>
      Layout Shell
      <Outlet />
    </div>
  ),
}))

vi.mock('../../components/ui/LoadingSpinner', () => ({
  LoadingSpinner: () => <div>Loading...</div>,
}))

const pages = [
  'Login',
  'Dashboard',
  'Wallets',
  'Trades',
  'Config',
  'Incidents',
  'Scout',
  'Signals',
  'Market',
  'Risk',
  'Reconciliation',
  'Performance',
  'Operations',
  'Consensus',
  'WalletMonitoring',
  'Webhooks',
  'RiskDashboard',
  'SignalsDashboard',
  'ScoutDashboard',
]

vi.mock('../../pages/Login', () => ({ Login: () => <div>Login page</div> }))
vi.mock('../../pages/Dashboard', () => ({ Dashboard: () => <div>Dashboard page</div> }))
vi.mock('../../pages/Wallets', () => ({ Wallets: () => <div>Wallets page</div> }))
vi.mock('../../pages/Trades', () => ({ Trades: () => <div>Trades page</div> }))
vi.mock('../../pages/Config', () => ({ Config: () => <div>Config page</div> }))
vi.mock('../../pages/Incidents', () => ({ Incidents: () => <div>Incidents page</div> }))
vi.mock('../../pages/Scout', () => ({ Scout: () => <div>Scout page</div> }))
vi.mock('../../pages/Signals', () => ({ Signals: () => <div>Signals page</div> }))
vi.mock('../../pages/Market', () => ({ Market: () => <div>Market page</div> }))
vi.mock('../../pages/Risk', () => ({ Risk: () => <div>Risk page</div> }))
vi.mock('../../pages/Reconciliation', () => ({ Reconciliation: () => <div>Reconciliation page</div> }))
vi.mock('../../pages/Performance', () => ({ Performance: () => <div>Performance page</div> }))
vi.mock('../../pages/Operations', () => ({ Operations: () => <div>Operations page</div> }))
vi.mock('../../pages/Consensus', () => ({ Consensus: () => <div>Consensus page</div> }))
vi.mock('../../pages/WalletMonitoring', () => ({ WalletMonitoring: () => <div>WalletMonitoring page</div> }))
vi.mock('../../pages/Webhooks', () => ({ Webhooks: () => <div>Webhooks page</div> }))
vi.mock('../../pages/RiskDashboard', () => ({ RiskDashboard: () => <div>RiskDashboard page</div> }))
vi.mock('../../pages/SignalsDashboard', () => ({ SignalsDashboard: () => <div>SignalsDashboard page</div> }))
vi.mock('../../pages/ScoutDashboard', () => ({ ScoutDashboard: () => <div>ScoutDashboard page</div> }))

function login() {
  useAuthStore.setState({
    user: { identifier: 'admin', role: 'admin', token: 'tok' },
    isAuthenticated: true,
    tokenExpiresAt: Date.now() + 3600000,
    refreshToken: null,
    lastActivity: Date.now(),
  })
}

function logout() {
  useAuthStore.setState({
    user: null,
    isAuthenticated: false,
    tokenExpiresAt: null,
    refreshToken: null,
    lastActivity: null,
  })
}

beforeEach(() => {
  logout()
})

async function renderAt(path: string) {
  render(
    <MemoryRouter initialEntries={[path]}>
      <App />
    </MemoryRouter>
  )
  await waitFor(() => expect(screen.queryByText('Loading...')).not.toBeInTheDocument())
}

describe('App', () => {
  it('renders the login page on /login', async () => {
    await renderAt('/login')
    expect(screen.getByText('Login page')).toBeInTheDocument()
  })

  it('redirects / to the dashboard', async () => {
    login()
    await renderAt('/')
    expect(screen.getByText('Dashboard page')).toBeInTheDocument()
  })

  it('redirects to login when unauthenticated', async () => {
    await renderAt('/dashboard')
    expect(screen.getByText('Login page')).toBeInTheDocument()
  })

  it('renders every protected route', async () => {
    login()
    for (const page of pages.filter((p) => p !== 'Login')) {
      const path =
        page === 'WalletMonitoring'
          ? '/wallet-monitoring'
          : page === 'RiskDashboard'
            ? '/risk-dashboard'
            : page === 'SignalsDashboard'
              ? '/signals-dashboard'
              : page === 'ScoutDashboard'
                ? '/scout-dashboard'
                : '/' + page.toLowerCase()
      await renderAt(path)
      expect(screen.getByText(`${page} page`)).toBeInTheDocument()
    }
  })

  it('blocks the config route without admin role', async () => {
    useAuthStore.setState({
      user: { identifier: 'op', role: 'operator', token: 'tok' },
      isAuthenticated: true,
      tokenExpiresAt: Date.now() + 3600000,
      refreshToken: null,
      lastActivity: Date.now(),
    })
    await renderAt('/config')
    expect(screen.queryByText('Config page')).not.toBeInTheDocument()
  })

  it('renders the config route for admins', async () => {
    login()
    await renderAt('/config')
    expect(screen.getByText('Config page')).toBeInTheDocument()
  })

  it('redirects unknown paths to the dashboard', async () => {
    login()
    await renderAt('/nonsense')
    expect(screen.getByText('Dashboard page')).toBeInTheDocument()
  })
})
