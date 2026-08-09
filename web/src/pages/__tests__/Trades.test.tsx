import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { Trades } from '../Trades'

const apiMock = vi.hoisted(() => ({
  useTrades: vi.fn(),
  exportTrades: vi.fn(),
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

function exportButton(label: string): HTMLElement {
  const btn = screen
    .getAllByRole('button')
    .find((b) => b.textContent?.includes(label))
  if (!btn) throw new Error(`Export button ${label} not found`)
  return btn
}

function makeTrade(overrides: Record<string, unknown> = {}) {
  return {
    id: 1,
    trade_uuid: 'trade-1',
    wallet_address: 'w1',
    token_address: 'tok-1',
    token_symbol: 'T1',
    strategy: 'SHIELD',
    side: 'BUY',
    amount_sol: '1.5',
    price_at_signal: '2.5',
    tx_signature: 'sig-1',
    status: 'CLOSED',
    retry_count: 0,
    error_message: null,
    pnl_sol: '0.25',
    pnl_usd: '0.5',
    created_at: '2025-01-01T10:00:00Z',
    updated_at: '2025-01-01T10:00:00Z',
    ...overrides,
  }
}

function setup(data: Record<string, unknown>, isLoading = false) {
  apiMock.useTrades.mockReturnValue({
    data,
    isLoading,
    refetch: vi.fn(),
  })
  apiMock.exportTrades.mockResolvedValue(undefined)
}

beforeEach(() => {
  vi.clearAllMocks()
  setup({
    trades: [
      makeTrade(),
      makeTrade({
        id: 2,
        trade_uuid: 'trade-2',
        token_symbol: null,
        side: 'SELL',
        status: 'FAILED',
        tx_signature: null,
        error_message: 'insufficient liquidity',
        pnl_sol: null,
        price_at_signal: null,
      }),
      makeTrade({
        id: 3,
        trade_uuid: 'trade-3',
        strategy: 'EXIT',
        status: 'CLOSED',
        tx_signature: null,
        pnl_sol: null,
      }),
    ],
    total: 3,
    limit: 25,
    offset: 0,
  })
})

describe('Trades', () => {
  it('renders the trade table with summary stats', () => {
    render(<Trades />)
    expect(screen.getByText('Total Trades:')).toBeInTheDocument()
    expect(screen.getAllByText('$T1').length).toBeGreaterThan(0)
    expect(screen.getByText('$Unknown')).toBeInTheDocument()
    expect(screen.getByText('insufficient liquidity')).toBeInTheDocument()
    expect(screen.getByText('Needs Review')).toBeInTheDocument()
    expect(screen.getByText('Verified')).toBeInTheDocument()
  })

  it('renders the loading state', () => {
    setup({ trades: [], total: 0, limit: 25, offset: 0 }, true)
    render(<Trades />)
    expect(screen.getByText('Loading trades...')).toBeInTheDocument()
  })

  it('renders the empty state', () => {
    setup({ trades: [], total: 0, limit: 25, offset: 0 })
    render(<Trades />)
    expect(screen.getByText('No trades found')).toBeInTheDocument()
  })

  it('applies date presets and custom dates', () => {
    render(<Trades />)
    fireEvent.click(screen.getByText('Today'))
    fireEvent.click(screen.getByText('7D'))
    fireEvent.click(screen.getByText('30D'))
    fireEvent.click(screen.getByText('Custom'))
    const from = document.getElementById('trades-date-from') as HTMLInputElement
    const to = document.getElementById('trades-date-to') as HTMLInputElement
    fireEvent.change(from, { target: { value: '2025-01-01' } })
    expect(apiMock.useTrades).toHaveBeenCalled()
    fireEvent.change(to, { target: { value: '2025-01-10' } })
    expect(screen.getByText('to')).toBeInTheDocument()
  })

  it('filters by strategy and status chips', () => {
    render(<Trades />)
    fireEvent.change(screen.getByText('All Strategies').closest('select') as HTMLSelectElement, {
      target: { value: 'SHIELD' },
    })
    fireEvent.click(screen.getByText('Dead Letter'))
    expect(screen.getByText('Dead Letter')).toBeInTheDocument()
    // click again to clear
    fireEvent.click(screen.getByText('Dead Letter'))
    fireEvent.click(screen.getByText('Clear'))
  })

  it('exports trades successfully', async () => {
    render(<Trades />)
    fireEvent.click(exportButton('CSV'))
    await waitFor(() => {
      expect(apiMock.exportTrades).toHaveBeenCalledWith(expect.anything(), 'csv')
    })
    expect(toastMock.success).toHaveBeenCalledWith('Trades exported as CSV successfully')

    fireEvent.click(exportButton('PDF'))
    await waitFor(() => {
      expect(apiMock.exportTrades).toHaveBeenCalledWith(expect.anything(), 'pdf')
    })
  })

  it('shows an error toast when the export fails', async () => {
    apiMock.exportTrades.mockRejectedValue(new Error('export failed'))
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(<Trades />)
    fireEvent.click(exportButton('CSV'))
    await waitFor(() => {
      expect(toastMock.error).toHaveBeenCalledWith('Failed to export trades. Please try again.')
    })
    errorSpy.mockRestore()
  })

  it('renders pagination when there are multiple pages', () => {
    const trades = Array.from({ length: 30 }, (_, i) => makeTrade({ id: i, trade_uuid: `t${i}` }))
    setup({ trades, total: 30, limit: 25, offset: 0 })
    render(<Trades />)
    expect(screen.getByText('Page 1 of 2')).toBeInTheDocument()
    fireEvent.click(screen.getByText('Next'))
    expect(screen.getByText('Page 2 of 2')).toBeInTheDocument()
    fireEvent.click(screen.getByText('Previous'))
    expect(screen.getByText('Page 1 of 2')).toBeInTheDocument()
  })

  it('renders trades without signatures as a dash', () => {
    const trades = [
      makeTrade({ id: 9, trade_uuid: 't9', status: 'PENDING', tx_signature: null }),
    ]
    setup({ trades, total: 1, limit: 25, offset: 0 })
    render(<Trades />)
    expect(screen.getByText('No signature')).toBeInTheDocument()
  })

  it('shows needs-reconciliation count when present', () => {
    const trades = [
      makeTrade({ id: 4, trade_uuid: 't4', status: 'CLOSED', tx_signature: null, error_message: null }),
    ]
    setup({ trades, total: 1, limit: 25, offset: 0 })
    render(<Trades />)
    expect(screen.getByText('Needs Reconciliation:')).toBeInTheDocument()
  })
})
