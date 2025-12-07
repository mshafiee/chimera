import { useState } from 'react'
import { ExternalLink, Download, ChevronLeft, ChevronRight, X } from 'lucide-react'
import { Card } from '../components/ui/Card'
import { Button } from '../components/ui/Button'
import { Badge, StatusBadge, StrategyBadge } from '../components/ui/Badge'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../components/ui/Table'
import { useTrades, exportTrades } from '../api'
import { toast } from '../components/ui/Toast'

const PAGE_SIZE = 25

// Helper to get Solana Explorer URL
const getSolanaExplorerUrl = (signature: string) => {
  return `https://explorer.solana.com/tx/${signature}?cluster=mainnet-beta`
}

// Helper to get Solscan URL
const getSolscanUrl = (signature: string) => {
  return `https://solscan.io/tx/${signature}`
}

export function Trades() {
  const [page, setPage] = useState(0)
  const [dateFrom, setDateFrom] = useState('')
  const [dateTo, setDateTo] = useState('')
  const [statusFilter, setStatusFilter] = useState('')
  const [strategyFilter, setStrategyFilter] = useState('')
  const [isExporting, setIsExporting] = useState(false)
  const [selectedDatePreset, setSelectedDatePreset] = useState<string>('')

  const { data: tradesData, isLoading } = useTrades({
    from: dateFrom || undefined,
    to: dateTo || undefined,
    status: statusFilter || undefined,
    strategy: strategyFilter || undefined,
    limit: PAGE_SIZE,
    offset: page * PAGE_SIZE,
  })

  const trades = tradesData?.trades || []
  const total = tradesData?.total || 0
  const totalPages = Math.ceil(total / PAGE_SIZE)

  const handleExport = async (format: 'csv' | 'pdf' = 'csv') => {
    setIsExporting(true)
    try {
      await exportTrades(
        {
          from: dateFrom || undefined,
          to: dateTo || undefined,
          status: statusFilter || undefined,
          strategy: strategyFilter || undefined,
        },
        format
      )
      toast.success(`Trades exported as ${format.toUpperCase()} successfully`)
    } catch (error) {
      console.error('Failed to export trades:', error)
      toast.error('Failed to export trades. Please try again.')
    }
    setIsExporting(false)
  }

  const clearFilters = () => {
    setDateFrom('')
    setDateTo('')
    setStatusFilter('')
    setStrategyFilter('')
    setSelectedDatePreset('')
    setPage(0)
  }

  // Date range presets
  const applyDatePreset = (preset: 'today' | '7d' | '30d' | 'custom') => {
    const now = new Date()
    const today = new Date(now.getFullYear(), now.getMonth(), now.getDate())
    
    switch (preset) {
      case 'today':
        setDateFrom(today.toISOString().split('T')[0])
        setDateTo(today.toISOString().split('T')[0])
        setSelectedDatePreset('today')
        break
      case '7d':
        const sevenDaysAgo = new Date(today)
        sevenDaysAgo.setDate(sevenDaysAgo.getDate() - 7)
        setDateFrom(sevenDaysAgo.toISOString().split('T')[0])
        setDateTo(today.toISOString().split('T')[0])
        setSelectedDatePreset('7d')
        break
      case '30d':
        const thirtyDaysAgo = new Date(today)
        thirtyDaysAgo.setDate(thirtyDaysAgo.getDate() - 30)
        setDateFrom(thirtyDaysAgo.toISOString().split('T')[0])
        setDateTo(today.toISOString().split('T')[0])
        setSelectedDatePreset('30d')
        break
      case 'custom':
        setSelectedDatePreset('custom')
        break
    }
    setPage(0)
  }

  // Status filter options
  const statusOptions = [
    { value: '', label: 'All Statuses' },
    { value: 'ACTIVE', label: 'Active' },
    { value: 'CLOSED', label: 'Closed' },
    { value: 'FAILED', label: 'Failed' },
    { value: 'PENDING', label: 'Pending' },
    { value: 'EXECUTING', label: 'Executing' },
    { value: 'QUEUED', label: 'Queued' },
    { value: 'DEAD_LETTER', label: 'Dead Letter' },
  ]

  // Determine if trade needs reconciliation (CLOSED trades without tx_signature or with error)
  const needsReconciliation = (trade: typeof trades[0]) => {
    return trade.status === 'CLOSED' && (!trade.tx_signature || trade.error_message !== null)
  }

  return (
    <div className="space-y-6">
      {/* Filters - Mobile Optimized */}
      <div className="space-y-3 md:space-y-4">
        <div className="flex flex-col md:flex-row items-stretch md:items-center justify-between gap-3 md:gap-4">
          <div className="flex flex-col sm:flex-row items-stretch sm:items-center gap-2 md:gap-4">
            {/* Date Range Presets */}
            <div className="flex items-center gap-2">
              <span className="text-sm text-text-muted">Quick Range:</span>
              <div className="flex gap-1 rounded-lg border border-border overflow-hidden">
                {(['today', '7d', '30d'] as const).map((preset) => (
                  <button
                    key={preset}
                    onClick={() => applyDatePreset(preset)}
                    className={`px-3 py-1.5 text-xs font-medium transition-colors ${
                      selectedDatePreset === preset
                        ? 'bg-shield text-background'
                        : 'bg-surface text-text-muted hover:text-text hover:bg-surface-light'
                    }`}
                  >
                    {preset === 'today' ? 'Today' : preset === '7d' ? '7D' : '30D'}
                  </button>
                ))}
                <button
                  onClick={() => applyDatePreset('custom')}
                  className={`px-3 py-1.5 text-xs font-medium transition-colors ${
                    selectedDatePreset === 'custom'
                      ? 'bg-shield text-background'
                      : 'bg-surface text-text-muted hover:text-text hover:bg-surface-light'
                  }`}
                >
                  Custom
                </button>
              </div>
            </div>

            {/* Date Range Inputs */}
            {(selectedDatePreset === 'custom' || (!selectedDatePreset && (dateFrom || dateTo))) && (
              <div className="flex items-center gap-2">
                <input
                  type="date"
                  value={dateFrom}
                  onChange={(e) => {
                    setDateFrom(e.target.value)
                    setSelectedDatePreset('custom')
                    setPage(0)
                  }}
                  className="bg-surface border border-border rounded-lg px-3 py-2 text-sm text-text focus:outline-none focus:ring-2 focus:ring-shield"
                  placeholder="From"
                />
                <span className="text-text-muted">to</span>
                <input
                  type="date"
                  value={dateTo}
                  onChange={(e) => {
                    setDateTo(e.target.value)
                    setSelectedDatePreset('custom')
                    setPage(0)
                  }}
                  className="bg-surface border border-border rounded-lg px-3 py-2 text-sm text-text focus:outline-none focus:ring-2 focus:ring-shield"
                  placeholder="To"
                />
              </div>
            )}

            {/* Strategy Filter */}
            <select
              value={strategyFilter}
              onChange={(e) => {
                setStrategyFilter(e.target.value)
                setPage(0)
              }}
              className="bg-surface border border-border rounded-lg px-3 py-2 text-sm text-text focus:outline-none focus:ring-2 focus:ring-shield"
            >
              <option value="">All Strategies</option>
              <option value="SHIELD">Shield</option>
              <option value="SPEAR">Spear</option>
              <option value="EXIT">Exit</option>
            </select>

            {/* Clear Filters */}
            {(dateFrom || dateTo || statusFilter || strategyFilter) && (
              <Button variant="ghost" size="sm" onClick={clearFilters}>
                <X className="w-4 h-4 mr-1" />
                Clear
              </Button>
            )}
          </div>

        {/* Export Buttons - Stack on mobile */}
        <div className="flex flex-col sm:flex-row items-stretch sm:items-center gap-2 w-full sm:w-auto">
          <Button
            variant="secondary"
            onClick={() => handleExport('csv')}
            loading={isExporting}
            disabled={trades.length === 0}
            className="w-full sm:w-auto"
          >
            <Download className="w-4 h-4 mr-2" />
            <span className="hidden sm:inline">Export </span>CSV
          </Button>
          <Button
            variant="secondary"
            onClick={() => handleExport('pdf')}
            loading={isExporting}
            disabled={trades.length === 0}
            className="w-full sm:w-auto"
          >
            <Download className="w-4 h-4 mr-2" />
            <span className="hidden sm:inline">Export </span>PDF
          </Button>
        </div>
      </div>

      {/* Status Filter Chips */}
      <div className="flex items-center gap-2 flex-wrap">
        <span className="text-sm text-text-muted">Status:</span>
        {statusOptions.slice(1).map((option) => (
          <button
            key={option.value}
            onClick={() => {
              setStatusFilter(statusFilter === option.value ? '' : option.value)
              setPage(0)
            }}
            className={`px-3 py-1.5 text-xs font-medium rounded-lg border transition-colors ${
              statusFilter === option.value
                ? 'bg-shield text-background border-shield'
                : 'bg-surface text-text-muted border-border hover:bg-surface-light hover:text-text'
            }`}
          >
            {option.label}
          </button>
        ))}
      </div>

      {/* Summary Stats */}
      <div className="flex items-center gap-6 text-sm">
        <div>
          <span className="text-text-muted">Total Trades:</span>{' '}
          <span className="font-mono-numbers">{total}</span>
        </div>
        <div>
          <span className="text-text-muted">Showing:</span>{' '}
          <span className="font-mono-numbers">
            {trades.length > 0 ? `${page * PAGE_SIZE + 1}-${page * PAGE_SIZE + trades.length}` : '0'}
          </span>
        </div>
        {trades.some(needsReconciliation) && (
          <div>
            <span className="text-text-muted">Needs Reconciliation:</span>{' '}
            <span className="font-mono-numbers text-warning">
              {trades.filter(needsReconciliation).length}
            </span>
          </div>
        )}
      </div>

      {/* Trades Table - Mobile Scrollable */}
      <Card padding="none">
        {isLoading ? (
          <div className="p-6 md:p-8 text-center text-text-muted text-sm">Loading trades...</div>
        ) : trades.length === 0 ? (
          <div className="p-6 md:p-8 text-center text-text-muted text-sm">No trades found</div>
        ) : (
          <div className="overflow-x-auto -mx-4 md:mx-0">
          <div className="inline-block min-w-full align-middle px-4 md:px-0">
          <Table>
            <TableHeader>
              <TableRow hoverable={false}>
                <TableHead>Time</TableHead>
                <TableHead>Token</TableHead>
                <TableHead>Strategy</TableHead>
                <TableHead>Side</TableHead>
                <TableHead>Amount</TableHead>
                <TableHead>Price</TableHead>
                <TableHead>PnL</TableHead>
                <TableHead>Status</TableHead>
                <TableHead>Transaction</TableHead>
                <TableHead>Reconciliation</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {trades.map((trade) => (
                <TableRow key={trade.trade_uuid}>
                  <TableCell>
                    <div className="text-sm">{formatTime(trade.created_at)}</div>
                    <div className="text-xs text-text-muted">
                      {formatDate(trade.created_at)}
                    </div>
                  </TableCell>
                  <TableCell>
                    <div className="font-semibold">
                      ${trade.token_symbol || 'Unknown'}
                    </div>
                    <div className="text-xs text-text-muted">
                      {trade.token_address.slice(0, 8)}...
                    </div>
                  </TableCell>
                  <TableCell>
                    <StrategyBadge strategy={trade.strategy} />
                  </TableCell>
                  <TableCell>
                    <Badge
                      variant={trade.side === 'BUY' ? 'success' : 'danger'}
                      size="sm"
                    >
                      {trade.side}
                    </Badge>
                  </TableCell>
                  <TableCell mono>{trade.amount_sol.toFixed(4)} SOL</TableCell>
                  <TableCell mono>
                    {trade.price_at_signal?.toFixed(8) || '-'}
                  </TableCell>
                  <TableCell mono>
                    {trade.pnl_sol !== null ? (
                      <div>
                        <span
                          className={trade.pnl_sol >= 0 ? 'text-profit' : 'text-loss'}
                        >
                          {trade.pnl_sol >= 0 ? '+' : ''}
                          {trade.pnl_sol.toFixed(4)} SOL
                        </span>
                        {trade.pnl_usd !== null && (
                          <div className="text-xs text-text-muted">
                            ${trade.pnl_usd.toFixed(2)}
                          </div>
                        )}
                      </div>
                    ) : (
                      '-'
                    )}
                  </TableCell>
                  <TableCell>
                    <StatusBadge status={trade.status} />
                    {trade.error_message && (
                      <div className="text-xs text-loss mt-1 truncate max-w-[150px]">
                        {trade.error_message}
                      </div>
                    )}
                  </TableCell>
                  <TableCell>
                    {trade.tx_signature ? (
                      <div className="flex items-center gap-2">
                        <a
                          href={getSolscanUrl(trade.tx_signature)}
                          target="_blank"
                          rel="noopener noreferrer"
                          className="text-shield hover:text-shield-dark transition-colors"
                          title="View on Solscan"
                        >
                          <ExternalLink className="w-4 h-4" />
                        </a>
                        <a
                          href={getSolanaExplorerUrl(trade.tx_signature)}
                          target="_blank"
                          rel="noopener noreferrer"
                          className="text-text-muted hover:text-text transition-colors"
                          title="View on Solana Explorer"
                        >
                          <ExternalLink className="w-3.5 h-3.5" />
                        </a>
                        <div className="text-xs text-text-muted font-mono">
                          {trade.tx_signature.slice(0, 4)}...{trade.tx_signature.slice(-4)}
                        </div>
                      </div>
                    ) : (
                      <span className="text-xs text-text-muted">No signature</span>
                    )}
                  </TableCell>
                  <TableCell>
                    {needsReconciliation(trade) ? (
                      <Badge variant="warning" size="sm" title="This trade needs reconciliation">
                        Needs Review
                      </Badge>
                    ) : trade.tx_signature ? (
                      <Badge variant="success" size="sm" title="Transaction verified on-chain">
                        Verified
                      </Badge>
                    ) : trade.status === 'CLOSED' ? (
                      <Badge variant="danger" size="sm" title="Closed trade missing transaction signature">
                        Missing TX
                      </Badge>
                    ) : (
                      <span className="text-xs text-text-muted">-</span>
                    )}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
          </div>
          </div>
        )}
      </Card>

      {/* Pagination */}
      {totalPages > 1 && (
        <div className="flex items-center justify-between">
          <div className="text-sm text-text-muted">
            Page {page + 1} of {totalPages}
          </div>
          <div className="flex gap-2">
            <Button
              variant="secondary"
              size="sm"
              onClick={() => setPage((p) => Math.max(0, p - 1))}
              disabled={page === 0}
            >
              <ChevronLeft className="w-4 h-4" />
              Previous
            </Button>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => setPage((p) => Math.min(totalPages - 1, p + 1))}
              disabled={page >= totalPages - 1}
            >
              Next
              <ChevronRight className="w-4 h-4" />
            </Button>
          </div>
        </div>
      )}
    </div>
  )
}

function formatTime(dateStr: string): string {
  const date = new Date(dateStr)
  return date.toLocaleTimeString('en-US', {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  })
}

function formatDate(dateStr: string): string {
  const date = new Date(dateStr)
  return date.toLocaleDateString('en-US', {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
  })
}
