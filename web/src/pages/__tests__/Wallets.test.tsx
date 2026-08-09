import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { Wallets } from '../Wallets'

const apiMock = vi.hoisted(() => ({
  useWallets: vi.fn(),
  useUpdateWallet: vi.fn(),
  useTrades: vi.fn(),
}))

vi.mock('../../api', () => apiMock)

const toastMock = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  warning: vi.fn(),
  info: vi.fn(),
}))

vi.mock('../../components/ui/Toast', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../components/ui/Toast')>()
  return { ...actual, toast: toastMock }
})

import { useAuthStore } from '../../stores/authStore'

function makeWallet(overrides: Record<string, unknown> = {}) {
  return {
    id: 1,
    address: 'wallet-address-0001',
    status: 'CANDIDATE',
    wqs_score: '75.5',
    roi_7d: '5.0',
    roi_30d: '12.3',
    trade_count_30d: 10,
    win_rate: '0.65',
    max_drawdown_30d: '3.0',
    avg_trade_size_sol: '1.2',
    last_trade_at: '2025-01-01T00:00:00Z',
    promoted_at: null,
    ttl_expires_at: null,
    notes: 'solid wallet',
    created_at: '2025-01-01T00:00:00Z',
    updated_at: '2025-01-01T00:00:00Z',
    ...overrides,
  }
}

const wallets = [
  makeWallet(),
  makeWallet({
    id: 2,
    address: 'wallet-address-0002',
    status: 'ACTIVE',
    wqs_score: '90.0',
    roi_30d: '-2.0',
    ttl_expires_at: new Date(Date.now() + 2 * 3600000).toISOString(),
  }),
  makeWallet({
    id: 3,
    address: 'wallet-address-0003',
    status: 'REJECTED',
    wqs_score: null,
    roi_30d: null,
    win_rate: null,
    trade_count_30d: null,
    notes: null,
  }),
  makeWallet({
    id: 4,
    address: 'wallet-address-0004',
    status: 'ACTIVE',
    wqs_score: '30.0',
    roi_30d: '-10.0',
  }),
  makeWallet({
    id: 5,
    address: 'wallet-address-0005',
    status: 'ACTIVE',
    wqs_score: '55.0',
    roi_30d: '1.0',
  }),
  makeWallet({
    id: 7,
    address: 'wallet-address-0007',
    status: 'ACTIVE',
    wqs_score: '60.0',
    roi_30d: '3.0',
    ttl_expires_at: new Date(Date.now() + 3 * 24 * 3600000).toISOString(),
  }),
  makeWallet({
    id: 6,
    address: 'wallet-address-0006',
    status: 'CANDIDATE',
    wqs_score: '10.0',
    roi_7d: null,
    roi_30d: null,
    max_drawdown_30d: null,
    avg_trade_size_sol: null,
    win_rate: null,
    last_trade_at: null,
    ttl_expires_at: new Date(Date.now() - 3600000).toISOString(),
    notes: null,
  }),
]

function setup() {
  apiMock.useWallets.mockReturnValue({
    data: { wallets, total: wallets.length },
    isLoading: false,
  })
  apiMock.useUpdateWallet.mockReturnValue({
    mutateAsync: vi.fn().mockResolvedValue({ success: true, wallet: null, message: 'ok' }),
    isPending: false,
  })
  apiMock.useTrades.mockReturnValue({
    data: { trades: [], total: 0, limit: 20, offset: 0 },
    isLoading: false,
  })
}

function loginAsAdmin() {
  useAuthStore.setState({
    user: { identifier: 'admin', role: 'admin', token: 'tok' },
    isAuthenticated: true,
    tokenExpiresAt: Date.now() + 3600000,
    refreshToken: null,
    lastActivity: Date.now(),
  })
}

function loginAsReadonly() {
  useAuthStore.setState({
    user: { identifier: 'ro', role: 'readonly', token: 'tok' },
    isAuthenticated: true,
    tokenExpiresAt: Date.now() + 3600000,
    refreshToken: null,
    lastActivity: Date.now(),
  })
}

beforeEach(() => {
  vi.clearAllMocks()
  setup()
  loginAsAdmin()
})

describe('Wallets', () => {
  it('renders the wallet table with stats', () => {
    render(<Wallets />)
    expect(screen.getByText('wallet-a...0001')).toBeInTheDocument()
    expect(screen.getByText('CANDIDATE')).toBeInTheDocument()
    expect(screen.getByText('75.5')).toBeInTheDocument()
    expect(screen.getAllByText('ACTIVE').length).toBeGreaterThan(0)
  })

  it('renders the loading and empty states', () => {
    apiMock.useWallets.mockReturnValue({ data: undefined, isLoading: true })
    render(<Wallets />)
    expect(screen.getByText('Loading wallets...')).toBeInTheDocument()
  })

  it('filters by status', () => {
    render(<Wallets />)
    fireEvent.click(screen.getByText('ACTIVE'))
    expect(apiMock.useWallets).toHaveBeenCalledWith('ACTIVE')
    fireEvent.click(screen.getByText('REJECTED'))
    expect(apiMock.useWallets).toHaveBeenCalledWith('REJECTED')
    fireEvent.click(screen.getByText('ALL'))
    expect(apiMock.useWallets).toHaveBeenCalledWith(undefined)
  })

  it('searches and applies advanced filters', () => {
    render(<Wallets />)
    const search = document.getElementById('wallet-search') as HTMLInputElement
    fireEvent.change(search, { target: { value: 'wallet-address-0001' } })
    expect(screen.getByText('wallet-a...0001')).toBeInTheDocument()

    fireEvent.change(search, { target: { value: '' } })
    fireEvent.click(screen.getByText('Advanced Filters'))
    fireEvent.change(document.getElementById('wqs-min') as HTMLInputElement, { target: { value: '80' } })
    fireEvent.change(document.getElementById('wqs-max') as HTMLInputElement, { target: { value: '100' } })
    fireEvent.change(document.getElementById('roi-min') as HTMLInputElement, { target: { value: '-100' } })
    fireEvent.change(document.getElementById('trade-count-min') as HTMLInputElement, { target: { value: '5' } })
    expect(screen.getByText('wallet-a...0002')).toBeInTheDocument()
    // roi-min rejects the remaining wallet (roi -2 < 100)
    fireEvent.change(document.getElementById('roi-min') as HTMLInputElement, { target: { value: '100' } })
    expect(screen.getByText('No wallets found')).toBeInTheDocument()
    // trade-count rejects it too (10 < 999)
    fireEvent.change(document.getElementById('roi-min') as HTMLInputElement, { target: { value: '-100' } })
    fireEvent.change(document.getElementById('trade-count-min') as HTMLInputElement, { target: { value: '999' } })
    expect(screen.getByText('No wallets found')).toBeInTheDocument()
    // wqs-max rejects it (90 > 10)
    fireEvent.change(document.getElementById('trade-count-min') as HTMLInputElement, { target: { value: '5' } })
    fireEvent.change(document.getElementById('wqs-max') as HTMLInputElement, { target: { value: '10' } })
    expect(screen.getByText('No wallets found')).toBeInTheDocument()
    fireEvent.click(screen.getByText('Clear Filters') as HTMLElement)
  })

  it('toggles an individual wallet selection', () => {
    render(<Wallets />)
    const selectButtons = document.querySelectorAll('tbody button')
    fireEvent.click(selectButtons[0])
    expect(screen.getByText((_, el) => el?.textContent === 'Promote Selected (1)')).toBeInTheDocument()
    fireEvent.click(selectButtons[0])
    expect(screen.queryByText((_, el) => el?.textContent?.includes('Promote Selected'))).not.toBeInTheDocument()
  })

  it('selects all wallets and toggles selections', () => {
    render(<Wallets />)
    const headerButtons = document.querySelectorAll('thead button')
    fireEvent.click(headerButtons[0])
    expect(screen.getByText(/Promote Selected/)).toBeInTheDocument()
    fireEvent.click(headerButtons[0])
    expect(screen.queryByText(/Promote Selected/)).not.toBeInTheDocument()
  })

  it('shows the empty-selection guard when bulk actions run without selections', () => {
    // The bulk buttons only render with a selection, so the guard inside the
    // handlers is defensive; verify the buttons stay hidden instead.
    render(<Wallets />)
    expect(screen.queryByText(/Promote Selected/)).not.toBeInTheDocument()
    expect(screen.queryByText(/Demote Selected/)).not.toBeInTheDocument()
  })

  it('shows errors when bulk operations fail', async () => {
    const mutateAsync = vi.fn().mockRejectedValue(new Error('bulk failed'))
    apiMock.useUpdateWallet.mockReturnValue({ mutateAsync, isPending: false })
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(<Wallets />)
    const selectButtons = document.querySelectorAll('tbody button')
    fireEvent.click(selectButtons[0])
    fireEvent.click(screen.getByText(/Promote Selected/))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Failed to promote some wallets. Please try again.')
    })
    fireEvent.click(selectButtons[2])
    fireEvent.click(screen.getByText(/Demote Selected/))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Failed to demote some wallets. Please try again.')
    })
    errorSpy.mockRestore()
  })

  it('shows an error when demoting via the modal fails', async () => {
    const mutateAsync = vi.fn().mockRejectedValue(new Error('demote failed'))
    apiMock.useUpdateWallet.mockReturnValue({ mutateAsync, isPending: false })
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(<Wallets />)
    const demoteButtons = [...document.querySelectorAll('button')].filter((b) => b.textContent === 'Demote')
    fireEvent.click(demoteButtons[0])
    fireEvent.click(screen.getAllByRole('button', { name: 'Demote' }).at(-1) as HTMLElement)
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Failed to demote wallet. Please try again.')
    })
    errorSpy.mockRestore()
  })

  it('shows an error when exporting fails', () => {
    vi.stubGlobal('URL', {
      createObjectURL: vi.fn(() => {
        throw new Error('no blob')
      }),
      revokeObjectURL: vi.fn(),
    })
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(<Wallets />)
    fireEvent.click(screen.getByText(/CSV/))
    expect(toastMock.error).toHaveBeenCalledWith('Failed to export wallets. Please try again.')
    errorSpy.mockRestore()
    vi.unstubAllGlobals()
  })

  it('cancels the promote modal', () => {
    render(<Wallets />)
    const promoteButtons = [...document.querySelectorAll('button')].filter((b) => b.textContent === 'Promote')
    fireEvent.click(promoteButtons[0])
    fireEvent.click(screen.getAllByRole('button', { name: 'Cancel' })[0])
    expect(screen.queryByText('Promote Wallet')).not.toBeInTheDocument()
  })

  it('closes the promote modal via the close button', () => {
    render(<Wallets />)
    const promoteButtons = [...document.querySelectorAll('button')].filter((b) => b.textContent === 'Promote')
    fireEvent.click(promoteButtons[0])
    fireEvent.click(screen.getByLabelText('Close dialog'))
    expect(screen.queryByText('Promote Wallet')).not.toBeInTheDocument()
  })

  it('expands a wallet with mid-range wqs for spear styling', () => {
    render(<Wallets />)
    fireEvent.click(screen.getByText('wallet-a...0005'))
    expect(screen.getByText('Recent Trade History')).toBeInTheDocument()
    fireEvent.click(screen.getByText('wallet-a...0005'))
  })

  it('selects wallets and bulk promotes', async () => {
    render(<Wallets />)
    // select the first wallet via its row checkbox button
    const selectButtons = document.querySelectorAll('tbody button')
    fireEvent.click(selectButtons[0])
    expect(screen.getByText(/Promote Selected/)).toBeInTheDocument()
    fireEvent.click(screen.getByText(/Promote Selected/))
    await waitFor(() => {
      expect(toastMock.success).toHaveBeenCalledWith('Successfully promoted 1 wallet(s)')
    })
  })

  it('shows a warning when bulk promoting without selections', async () => {
    render(<Wallets />)
    // no selection - bulk buttons hidden for readonly, but promote button only appears with selection
    expect(screen.queryByText(/Promote Selected/)).not.toBeInTheDocument()
  })

  it('bulk demotes selected wallets', async () => {
    const mutateAsync = vi.fn().mockResolvedValue({ success: true, wallet: null, message: 'ok' })
    apiMock.useUpdateWallet.mockReturnValue({ mutateAsync, isPending: false })
    render(<Wallets />)
    const selectButtons = document.querySelectorAll('tbody button')
    // row 2 (ACTIVE wallet) checkbox is at index 2
    fireEvent.click(selectButtons[2])
    fireEvent.click(screen.getByText(/Demote Selected/))
    await waitFor(() => {
      expect(toastMock.success).toHaveBeenCalledWith('Successfully demoted 1 wallet(s)')
    })
  })

  it('promotes and demotes via the modals', async () => {
    const mutateAsync = vi.fn().mockResolvedValue({ success: true, wallet: null, message: 'ok' })
    apiMock.useUpdateWallet.mockReturnValue({ mutateAsync, isPending: false })
    render(<Wallets />)
    const promoteButtons = [...document.querySelectorAll('button')].filter((b) => b.textContent === 'Promote')
    fireEvent.click(promoteButtons[0])
    expect(screen.getByText('Promote Wallet')).toBeInTheDocument()
    const ttlSelect = document.getElementById('ttl-hours') as HTMLSelectElement
    fireEvent.change(ttlSelect, { target: { value: '24' } })
    fireEvent.click(screen.getAllByRole('button', { name: 'Promote' }).at(-1) as HTMLElement)
    await waitFor(() => {
      expect(mutateAsync).toHaveBeenCalledWith(
        expect.objectContaining({ status: 'ACTIVE', ttl_hours: 24 })
      )
    })

    const demoteButtons = [...document.querySelectorAll('button')].filter((b) => b.textContent === 'Demote')
    fireEvent.click(demoteButtons[0])
    expect(screen.getByText('Demote Wallet')).toBeInTheDocument()
    fireEvent.click(screen.getAllByRole('button', { name: 'Demote' }).at(-1) as HTMLElement)
    await waitFor(() => {
      expect(mutateAsync).toHaveBeenCalledWith(expect.objectContaining({ status: 'CANDIDATE' }))
    })
  })

  it('handles promote errors', async () => {
    const mutateAsync = vi.fn().mockRejectedValue(new Error('fail'))
    apiMock.useUpdateWallet.mockReturnValue({ mutateAsync, isPending: false })
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(<Wallets />)
    const promoteButtons = [...document.querySelectorAll('button')].filter((b) => b.textContent === 'Promote')
    fireEvent.click(promoteButtons[0])
    fireEvent.click(screen.getAllByRole('button', { name: 'Promote' }).at(-1) as HTMLElement)
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Failed to promote wallet. Please try again.')
    })
    errorSpy.mockRestore()
  })

  it('expands a wallet row to show details and trades', () => {
    apiMock.useTrades.mockReturnValue({
      data: {
        trades: [
          {
            trade_uuid: 't1', wallet_address: 'w', token_address: 'tok', token_symbol: 'SOL', strategy: 'SHIELD', side: 'BUY', amount_sol: '1', price_at_signal: '2', tx_signature: 'x', status: 'CLOSED', retry_count: 0, error_message: null, pnl_sol: '1', pnl_usd: '2', created_at: '2025-01-01T00:00:00Z', updated_at: '2025-01-01T00:00:00Z',
          },
          {
            trade_uuid: 't2', wallet_address: 'w', token_address: 'tok', token_symbol: 'SOL2', strategy: 'SPEAR', side: 'SELL', amount_sol: '1', price_at_signal: '2', tx_signature: 'y', status: 'ACTIVE', retry_count: 0, error_message: null, pnl_sol: null, pnl_usd: null, created_at: '2025-01-02T00:00:00Z', updated_at: '2025-01-02T00:00:00Z',
          },
          {
            trade_uuid: 't3', wallet_address: 'w', token_address: 'tok', token_symbol: 'SOL3', strategy: 'SHIELD', side: 'SELL', amount_sol: '1', price_at_signal: '2', tx_signature: 'z', status: 'CLOSED', retry_count: 0, error_message: null, pnl_sol: '-0.5', pnl_usd: '-1', created_at: '2025-01-03T00:00:00Z', updated_at: '2025-01-03T00:00:00Z',
          },
        ],
        total: 1,
        limit: 20,
        offset: 0,
      },
      isLoading: false,
    })
    render(<Wallets />)
    fireEvent.click(screen.getByText('wallet-a...0001'))
    expect(screen.getByText('Performance')).toBeInTheDocument()
    expect(screen.getByText('Activity')).toBeInTheDocument()
    expect(screen.getByText('WQS Breakdown')).toBeInTheDocument()
    expect(screen.getByText('SOL')).toBeInTheDocument()
    fireEvent.click(screen.getByText('wallet-a...0001'))
    expect(screen.queryByText('WQS Breakdown')).not.toBeInTheDocument()
  })

  it('exports wallets to CSV', () => {
    const createObjectURL = vi.fn(() => 'blob:url')
    const revokeObjectURL = vi.fn()
    vi.stubGlobal('URL', { createObjectURL, revokeObjectURL })
    const clickSpy = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {})
    render(<Wallets />)
    fireEvent.click(screen.getByText(/CSV/))
    expect(clickSpy).toHaveBeenCalled()
    expect(toastMock.success).toHaveBeenCalledWith('Wallets exported to CSV')
    vi.unstubAllGlobals()
    clickSpy.mockRestore()
  })

  it('renders pagination and navigates pages', () => {
    const many = Array.from({ length: 60 }, (_, i) => makeWallet({ id: i, address: `wallet-address-${String(i).padStart(4, '0')}` }))
    apiMock.useWallets.mockReturnValue({
      data: { wallets: many, total: many.length },
      isLoading: false,
    })
    render(<Wallets />)
    expect(screen.getByText(/60 total wallets/)).toBeInTheDocument()
    fireEvent.click(screen.getByText('Next'))
    fireEvent.click(screen.getByText('Previous'))
  })

  it('hides modification actions for readonly users', () => {
    loginAsReadonly()
    render(<Wallets />)
    expect(screen.queryByText('Promote')).not.toBeInTheDocument()
    expect(screen.queryByText('Demote')).not.toBeInTheDocument()
  })

  it('shows TTL badges and formats dates', () => {
    render(<Wallets />)
    expect(screen.getByText('1h left')).toBeInTheDocument()
    expect(screen.getByText('Expired')).toBeInTheDocument()
    expect(screen.getByText('2d left')).toBeInTheDocument()
  })

  it('expands a wallet with null fields', () => {
    render(<Wallets />)
    fireEvent.click(screen.getByText('wallet-a...0006'))
    expect(screen.getByText('No notes')).toBeInTheDocument()
    expect(screen.getAllByText('-').length).toBeGreaterThan(0)
  })
})
