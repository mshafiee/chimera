import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen } from '@testing-library/react'
import { WalletMonitoring } from '../WalletMonitoring'

const apiMock = vi.hoisted(() => ({ useWalletMonitoringStates: vi.fn() }))

vi.mock('../../api', () => apiMock)

const states = [
  { address: 'wallet-address-1', method: 'webhook', status: 'active', last_activity: 'x', last_fetch: '2025-01-01T00:00:00Z', failed_fetches: 0, success_rate: 99.5, next_fetch: '2025-01-02T00:00:00Z' },
  { address: 'wallet-address-2', method: 'polling', status: 'inactive', last_activity: 'x', last_fetch: null, failed_fetches: 3, success_rate: 80, next_fetch: null },
  { address: 'wallet-address-3', method: 'webhook', status: 'error', last_activity: 'x', last_fetch: '2025-01-01T00:00:00Z', failed_fetches: 10, success_rate: 60, next_fetch: '2025-01-02T00:00:00Z' },
]

function setup() {
  apiMock.useWalletMonitoringStates.mockReturnValue({
    data: { wallet_states: states },
    isLoading: false,
  })
}

beforeEach(() => {
  vi.clearAllMocks()
  setup()
})

describe('WalletMonitoring', () => {
  it('renders the full page with metrics and table', () => {
    render(<WalletMonitoring />)
    expect(screen.getByText('Wallet Monitoring')).toBeInTheDocument()
    expect(screen.getByText('Active Monitors')).toBeInTheDocument()
    expect(screen.getAllByText('Webhook').length).toBeGreaterThan(0)
    expect(screen.getAllByText('Polling').length).toBeGreaterThan(0)
    expect(screen.getByText('Errors')).toBeInTheDocument()
    const tbody = document.querySelector('tbody')
    expect(tbody?.textContent).toContain('wallet-a')
    expect(tbody?.textContent).toContain('ddress-1')
    expect(screen.getByText('99.5%')).toBeInTheDocument()
    expect(screen.getByText('Never')).toBeInTheDocument()
    expect(screen.getByText('N/A')).toBeInTheDocument()
    expect(screen.getByText('Webhook Monitoring')).toBeInTheDocument()
    expect(screen.getByText('Polling Monitoring')).toBeInTheDocument()
  })

  it('renders the loading state', () => {
    apiMock.useWalletMonitoringStates.mockReturnValue({ data: undefined, isLoading: true })
    render(<WalletMonitoring />)
    expect(screen.getByText('Loading wallet monitoring states...')).toBeInTheDocument()
  })

  it('renders the empty state', () => {
    apiMock.useWalletMonitoringStates.mockReturnValue({
      data: { wallet_states: [] },
      isLoading: false,
    })
    render(<WalletMonitoring />)
    expect(screen.getByText('No wallet monitoring data available')).toBeInTheDocument()
  })
})
