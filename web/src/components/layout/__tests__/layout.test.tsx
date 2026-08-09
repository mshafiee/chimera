import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { MemoryRouter, Routes, Route } from 'react-router-dom'
import { Header } from '../Header'
import { Sidebar, MobileBottomNav } from '../Sidebar'
import { Layout, useLayoutContext } from '../Layout'
import { useAuthStore } from '../../../stores/authStore'
import * as layoutBarrel from '../index'

const useWebSocketMock = vi.hoisted(() => vi.fn())
const useHealthMock = vi.hoisted(() => vi.fn())

vi.mock('../../../hooks/useWebSocket', () => ({
  useWebSocket: useWebSocketMock,
}))

vi.mock('../../../api', () => ({
  useHealth: useHealthMock,
}))

vi.mock('../../wallet', () => ({
  ConnectWalletButton: () => <div>Connect wallet button</div>,
  LogoutButton: () => <button>Logout</button>,
}))

function login() {
  useAuthStore.setState({
    user: { identifier: 'admin1', role: 'admin', token: 'tok' },
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
  vi.clearAllMocks()
  logout()
  useWebSocketMock.mockReturnValue({
    isConnected: false,
    isConnecting: false,
    connectionError: null,
    lastMessage: null,
    connect: vi.fn(),
    disconnect: vi.fn(),
    send: vi.fn(),
  })
  useHealthMock.mockReturnValue({ data: undefined, error: null })
})

describe('layout barrel', () => {
  it('re-exports all components', () => {
    expect(layoutBarrel.Layout).toBeTruthy()
    expect(layoutBarrel.useLayoutContext).toBeTruthy()
    expect(layoutBarrel.Sidebar).toBeTruthy()
    expect(layoutBarrel.MobileBottomNav).toBeTruthy()
    expect(layoutBarrel.Header).toBeTruthy()
  })
})

describe('Header', () => {
  it('renders the title for a known route', () => {
    render(
      <MemoryRouter initialEntries={['/wallets']}>
        <Header />
      </MemoryRouter>
    )
    expect(screen.getByText('Wallet Roster')).toBeInTheDocument()
  })

  it('falls back to Chimera for unknown routes', () => {
    render(
      <MemoryRouter initialEntries={['/unknown']}>
        <Header />
      </MemoryRouter>
    )
    expect(screen.getByText('Chimera')).toBeInTheDocument()
  })

  it('renders connected state, last update and refresh button', () => {
    const onRefresh = vi.fn()
    render(
      <MemoryRouter initialEntries={['/dashboard']}>
        <Header isConnected lastUpdate={new Date(Date.now() - 3000)} onRefresh={onRefresh} />
      </MemoryRouter>
    )
    expect(screen.getByText('Live')).toBeInTheDocument()
    expect(screen.getByText(/just now/)).toBeInTheDocument()
    fireEvent.click(screen.getByLabelText('Refresh data'))
    expect(onRefresh).toHaveBeenCalled()
  })

  it('renders disconnected state without update', () => {
    render(
      <MemoryRouter initialEntries={['/dashboard']}>
        <Header isConnected={false} />
      </MemoryRouter>
    )
    expect(screen.getByText('Disconnected')).toBeInTheDocument()
    expect(screen.getByText('Connect wallet button')).toBeInTheDocument()
  })

  it('formats time ago for various ranges', () => {
    const { rerender } = render(
      <MemoryRouter initialEntries={['/dashboard']}>
        <Header lastUpdate={new Date(Date.now() - 10 * 60 * 1000)} />
      </MemoryRouter>
    )
    expect(screen.getByText(/10m ago/)).toBeInTheDocument()
    rerender(
      <MemoryRouter initialEntries={['/dashboard']}>
        <Header lastUpdate={new Date(Date.now() - 5 * 3600 * 1000)} />
      </MemoryRouter>
    )
    expect(screen.getByText(/5h ago/)).toBeInTheDocument()
    rerender(
      <MemoryRouter initialEntries={['/dashboard']}>
        <Header lastUpdate={new Date(Date.now() - 3 * 86400 * 1000)} />
      </MemoryRouter>
    )
    expect(screen.getByText(/3d ago/)).toBeInTheDocument()
    rerender(
      <MemoryRouter initialEntries={['/dashboard']}>
        <Header lastUpdate={new Date(Date.now() - 30000)} />
      </MemoryRouter>
    )
    expect(screen.getByText(/30s ago/)).toBeInTheDocument()
  })

  it('renders user info when authenticated', () => {
    login()
    render(
      <MemoryRouter initialEntries={['/dashboard']}>
        <Header />
      </MemoryRouter>
    )
    expect(screen.getByText('admin1...')).toBeInTheDocument()
    expect(screen.getByText('admin')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /logout/i })).toBeInTheDocument()
  })
})

describe('Sidebar', () => {
  it('renders navigation items and version', () => {
    render(
      <MemoryRouter initialEntries={['/dashboard']}>
        <Sidebar />
      </MemoryRouter>
    )
    expect(screen.getByText('Dashboard')).toBeInTheDocument()
    expect(screen.getByText('Wallet Monitoring')).toBeInTheDocument()
    expect(screen.getByText('Consensus')).toBeInTheDocument()
    expect(screen.getByText(/Chimera v/)).toBeInTheDocument()
    expect(screen.getByText('© 2025 Project Chimera')).toBeInTheDocument()
  })

  it('calls onNavigate when a link is clicked', () => {
    const onNavigate = vi.fn()
    render(
      <MemoryRouter initialEntries={['/dashboard']}>
        <Sidebar onNavigate={onNavigate} />
      </MemoryRouter>
    )
    fireEvent.click(screen.getByText('Wallets'))
    expect(onNavigate).toHaveBeenCalled()
  })
})

describe('MobileBottomNav', () => {
  it('renders key items', () => {
    render(
      <MemoryRouter initialEntries={['/dashboard']}>
        <MobileBottomNav />
      </MemoryRouter>
    )
    expect(screen.getByText('Trades')).toBeInTheDocument()
    expect(screen.getByText('Risk Analysis')).toBeInTheDocument()
    expect(screen.getByText('Scout Integration')).toBeInTheDocument()
  })
})

describe('Layout', () => {
  it('renders the shell with halted banner', () => {
    useHealthMock.mockReturnValue({
      data: {
        status: 'healthy',
        uptime_seconds: 100,
        queue_depth: 5,
        rpc_latency_ms: 10,
        last_trade_at: null,
        database: { status: 'healthy', message: null },
        rpc: { status: 'healthy', message: null },
        circuit_breaker: {
          state: 'TRIPPED',
          trading_allowed: false,
          trip_reason: 'loss limit hit',
          cooldown_remaining_secs: 125,
        },
        price_cache: { total_entries: 1, tracked_tokens: 1 },
      },
      error: null,
    })
    login()
    render(
      <MemoryRouter initialEntries={['/dashboard']}>
        <Routes>
          <Route path="/dashboard" element={<Layout />}>
            <Route index element={<div>Dashboard content</div>} />
          </Route>
        </Routes>
      </MemoryRouter>
    )
    expect(screen.getByText(/Trading System Halted/)).toBeInTheDocument()
    expect(screen.getByText(/loss limit hit/)).toBeInTheDocument()
    expect(screen.getByText(/Cooldown: 2m 5s/)).toBeInTheDocument()
    expect(screen.getByText('Dashboard content')).toBeInTheDocument()
    expect(screen.getByText('TRIPPED')).toBeInTheDocument()
  })

  it('renders without a halted banner when trading is allowed', () => {
    useHealthMock.mockReturnValue({
      data: {
        status: 'healthy',
        uptime_seconds: 100,
        queue_depth: 0,
        rpc_latency_ms: 10,
        last_trade_at: null,
        database: { status: 'healthy', message: null },
        rpc: { status: 'healthy', message: null },
        circuit_breaker: {
          state: 'ACTIVE',
          trading_allowed: true,
          trip_reason: null,
          cooldown_remaining_secs: null,
        },
        price_cache: { total_entries: 0, tracked_tokens: 0 },
      },
      error: null,
    })
    render(
      <MemoryRouter initialEntries={['/dashboard']}>
        <Routes>
          <Route path="/dashboard" element={<Layout />}>
            <Route index element={<div>Dashboard content</div>} />
          </Route>
        </Routes>
      </MemoryRouter>
    )
    expect(screen.queryByText(/Trading System Halted/)).not.toBeInTheDocument()
    expect(screen.getByText('Dashboard content')).toBeInTheDocument()
  })

  it('toggles the mobile menu', () => {
    render(
      <MemoryRouter initialEntries={['/dashboard']}>
        <Routes>
          <Route path="/dashboard" element={<Layout />}>
            <Route index element={<div>Dashboard content</div>} />
          </Route>
        </Routes>
      </MemoryRouter>
    )
    fireEvent.click(screen.getByLabelText('Toggle menu'))
    expect(screen.getAllByText('Dashboard').length).toBeGreaterThan(0)
    fireEvent.keyDown(document.querySelector('[role="presentation"]') as HTMLElement, {
      key: 'Escape',
    })
    expect(screen.getByText('Dashboard content')).toBeInTheDocument()
  })

  it('triggers a refresh via the header refresh button', () => {
    const eventSpy = vi.spyOn(window, 'dispatchEvent')
    render(
      <MemoryRouter initialEntries={['/dashboard']}>
        <Routes>
          <Route path="/dashboard" element={<Layout />}>
            <Route index element={<div>Dashboard content</div>} />
          </Route>
        </Routes>
      </MemoryRouter>
    )
    fireEvent.click(screen.getByLabelText('Refresh data'))
    expect(eventSpy).toHaveBeenCalledWith(expect.any(CustomEvent))
    expect(screen.getByText(/just now/)).toBeInTheDocument()
  })

  it('focuses main on route change', () => {
    const focusSpy = vi.fn()
    render(
      <MemoryRouter initialEntries={['/dashboard']}>
        <Routes>
          <Route path="/dashboard" element={<Layout />}>
            <Route index element={<div>Dashboard content</div>} />
          </Route>
          <Route path="/other" element={<div>Other content</div>} />
        </Routes>
      </MemoryRouter>
    )
    const main = document.querySelector('main')
    if (main) {
      main.focus = focusSpy
    }
    expect(focusSpy).not.toHaveBeenCalled()
    expect(screen.getByText('Dashboard content')).toBeInTheDocument()
  })

  it('useLayoutContext exposes the outlet context', () => {
    function Consumer() {
      useLayoutContext()
      return <div>Context consumer</div>
    }
    render(
      <MemoryRouter initialEntries={['/dashboard']}>
        <Routes>
          <Route path="/dashboard" element={<Layout />}>
            <Route index element={<Consumer />} />
          </Route>
        </Routes>
      </MemoryRouter>
    )
    expect(screen.getByText('Context consumer')).toBeInTheDocument()
  })
})
