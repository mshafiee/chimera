import { useState } from 'react'
import { ExternalLink, Download, ChevronLeft, ChevronRight } from 'lucide-react'
import { Card } from '../components/ui/Card'
import { Button } from '../components/ui/Button'
import { Badge, StatusBadge, StrategyBadge } from '../components/ui/Badge'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../components/ui/Table'
import { useTrades, exportTrades } from '../api'

const PAGE_SIZE = 25

export function Trades() {
  const [page, setPage] = useState(0)
  const [dateFrom, setDateFrom] = useState('')
  const [dateTo, setDateTo] = useState('')
  const [statusFilter, setStatusFilter] = useState('')
  const [strategyFilter, setStrategyFilter] = useState('')
  const [isExporting, setIsExporting] = useState(false)

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

  const handleExport = async () => {
    setIsExporting(true)
    try {
      await exportTrades({
        from: dateFrom || undefined,
        to: dateTo || undefined,
        status: statusFilter || undefined,
        strategy: strategyFilter || undefined,
      })
    } catch (error) {
      console.error('Failed to export trades:', error)
    }
    setIsExporting(false)
  }

  const clearFilters = () => {
    setDateFrom('')
    setDateTo('')
    setStatusFilter('')
    setStrategyFilter('')
    setPage(0)
  }

  return (
    <div className="space-y-6">
      {/* Filters */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-4">
          {/* Date Range */}
          <div className="flex items-center gap-2">
            <input
              type="date"
              value={dateFrom}
              onChange={(e) => {
                setDateFrom(e.target.value)
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
                setPage(0)
              }}
              className="bg-surface border border-border rounded-lg px-3 py-2 text-sm text-text focus:outline-none focus:ring-2 focus:ring-shield"
              placeholder="To"
            />
          </div>

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

          {/* Status Filter */}
          <select
            value={statusFilter}
            onChange={(e) => {
              setStatusFilter(e.target.value)
              setPage(0)
            }}
            className="bg-surface border border-border rounded-lg px-3 py-2 text-sm text-text focus:outline-none focus:ring-2 focus:ring-shield"
          >
            <option value="">All Statuses</option>
            <option value="ACTIVE">Active</option>
            <option value="CLOSED">Closed</option>
            <option value="FAILED">Failed</option>
            <option value="PENDING">Pending</option>
            <option value="EXECUTING">Executing</option>
          </select>

          {/* Clear Filters */}
          {(dateFrom || dateTo || statusFilter || strategyFilter) && (
            <Button variant="ghost" size="sm" onClick={clearFilters}>
              Clear Filters
            </Button>
          )}
        </div>

        {/* Export Button */}
        <Button
          variant="secondary"
          onClick={handleExport}
          loading={isExporting}
          disabled={trades.length === 0}
        >
          <Download className="w-4 h-4 mr-2" />
          Export CSV
        </Button>
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
      </div>

      {/* Trades Table */}
      <Card padding="none">
        {isLoading ? (
          <div className="p-8 text-center text-text-muted">Loading trades...</div>
        ) : trades.length === 0 ? (
          <div className="p-8 text-center text-text-muted">No trades found</div>
        ) : (
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
                <TableHead></TableHead>
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
                    {trade.tx_signature && (
                      <a
                        href={`https://solscan.io/tx/${trade.tx_signature}`}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="text-shield hover:text-shield-dark"
                      >
                        <ExternalLink className="w-4 h-4" />
                      </a>
                    )}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
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
