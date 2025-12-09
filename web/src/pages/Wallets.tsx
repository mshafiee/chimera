import { useState } from 'react'
import { Search, ChevronDown, ChevronUp, Download, CheckSquare, Square, ExternalLink } from 'lucide-react'
import { Card } from '../components/ui/Card'
import { Button } from '../components/ui/Button'
import { Badge, StatusBadge } from '../components/ui/Badge'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../components/ui/Table'
import { Modal, ConfirmModal } from '../components/ui/Modal'
import { useWallets, useUpdateWallet, useTrades } from '../api'
import { useAuthStore } from '../stores/authStore'
import { toast } from '../components/ui/Toast'
import type { Wallet } from '../types'

type WalletStatus = 'ALL' | 'ACTIVE' | 'CANDIDATE' | 'REJECTED'

// Helper to get Solana Explorer URL for wallet address
const getWalletExplorerUrl = (address: string) => {
  // Detect network from RPC URL
  const rpcUrl = import.meta.env.VITE_SOLANA_RPC || ''
  const isDevnet = rpcUrl.includes('devnet') || rpcUrl.includes('testnet')
  const cluster = isDevnet ? 'devnet' : 'mainnet-beta'
  return `https://explorer.solana.com/address/${address}?cluster=${cluster}`
}

// Helper to get Solscan URL for wallet address
const getWalletSolscanUrl = (address: string) => {
  // Detect network from RPC URL
  const rpcUrl = import.meta.env.VITE_SOLANA_RPC || ''
  const isDevnet = rpcUrl.includes('devnet') || rpcUrl.includes('testnet')
  const network = isDevnet ? '?cluster=devnet' : ''
  return `https://solscan.io/account/${address}${network}`
}

export function Wallets() {
  const [statusFilter, setStatusFilter] = useState<WalletStatus>('ALL')
  const [searchQuery, setSearchQuery] = useState('')
  const [expandedWallet, setExpandedWallet] = useState<string | null>(null)
  const [promoteModal, setPromoteModal] = useState<Wallet | null>(null)
  const [demoteModal, setDemoteModal] = useState<Wallet | null>(null)
  const [ttlHours, setTtlHours] = useState<number | undefined>(undefined)
  const [selectedWallets, setSelectedWallets] = useState<Set<string>>(new Set())
  const [wqsMinFilter, setWqsMinFilter] = useState<number | undefined>(undefined)
  const [wqsMaxFilter, setWqsMaxFilter] = useState<number | undefined>(undefined)
  const [roiMinFilter, setRoiMinFilter] = useState<number | undefined>(undefined)
  const [tradeCountMinFilter, setTradeCountMinFilter] = useState<number | undefined>(undefined)

  const { hasPermission } = useAuthStore()
  const canModify = hasPermission('operator')

  const { data: walletsData, isLoading } = useWallets(
    statusFilter === 'ALL' ? undefined : statusFilter
  )
  const updateWallet = useUpdateWallet()

  const wallets = walletsData?.wallets || []

  // Filter by search query and advanced filters
  const filteredWallets = wallets.filter((wallet) => {
    // Search filter
    if (searchQuery && !wallet.address.toLowerCase().includes(searchQuery.toLowerCase())) {
      return false
    }
    
    // WQS range filter
    if (wqsMinFilter !== undefined && (wallet.wqs_score === null || wallet.wqs_score < wqsMinFilter)) {
      return false
    }
    if (wqsMaxFilter !== undefined && (wallet.wqs_score === null || wallet.wqs_score > wqsMaxFilter)) {
      return false
    }
    
    // ROI threshold filter
    if (roiMinFilter !== undefined && (wallet.roi_30d === null || wallet.roi_30d < roiMinFilter)) {
      return false
    }
    
    // Trade count filter
    if (tradeCountMinFilter !== undefined && (wallet.trade_count_30d === null || wallet.trade_count_30d < tradeCountMinFilter)) {
      return false
    }
    
    return true
  })

  // Toggle wallet selection
  const toggleWalletSelection = (address: string) => {
    const newSelected = new Set(selectedWallets)
    if (newSelected.has(address)) {
      newSelected.delete(address)
    } else {
      newSelected.add(address)
    }
    setSelectedWallets(newSelected)
  }

  // Toggle all wallets selection
  const toggleAllWallets = () => {
    if (selectedWallets.size === filteredWallets.length) {
      setSelectedWallets(new Set())
    } else {
      setSelectedWallets(new Set(filteredWallets.map(w => w.address)))
    }
  }

  // Bulk promote selected wallets
  const handleBulkPromote = async () => {
    if (selectedWallets.size === 0) {
      toast.warning('Please select wallets to promote')
      return
    }

    try {
      const promotePromises = Array.from(selectedWallets).map(address =>
        updateWallet.mutateAsync({
          address,
          status: 'ACTIVE',
          ttl_hours: ttlHours,
          reason: 'Bulk promotion via dashboard',
        })
      )
      await Promise.all(promotePromises)
      setSelectedWallets(new Set())
      setTtlHours(undefined)
      toast.success(`Successfully promoted ${selectedWallets.size} wallet(s)`)
    } catch (error) {
      console.error('Failed to bulk promote wallets:', error)
      toast.error('Failed to promote some wallets. Please try again.')
    }
  }

  // Bulk demote selected wallets
  const handleBulkDemote = async () => {
    if (selectedWallets.size === 0) {
      toast.warning('Please select wallets to demote')
      return
    }

    try {
      const demotePromises = Array.from(selectedWallets).map(address =>
        updateWallet.mutateAsync({
          address,
          status: 'CANDIDATE',
          reason: 'Bulk demotion via dashboard',
        })
      )
      await Promise.all(demotePromises)
      setSelectedWallets(new Set())
      toast.success(`Successfully demoted ${selectedWallets.size} wallet(s)`)
    } catch (error) {
      console.error('Failed to bulk demote wallets:', error)
      toast.error('Failed to demote some wallets. Please try again.')
    }
  }

  // Export wallets to CSV
  const handleExportCSV = async () => {
    try {
      const csvRows = [
        ['Address', 'Status', 'WQS Score', 'ROI 30d', 'Trade Count 30d', 'Win Rate', 'Max Drawdown', 'TTL Expires'].join(','),
        ...filteredWallets.map(wallet =>
          [
            wallet.address,
            wallet.status,
            wallet.wqs_score?.toFixed(2) || '',
            wallet.roi_30d?.toFixed(2) || '',
            wallet.trade_count_30d?.toString() || '',
            wallet.win_rate ? (wallet.win_rate * 100).toFixed(2) : '',
            wallet.max_drawdown_30d?.toFixed(2) || '',
            wallet.ttl_expires_at || '',
          ].join(',')
        ),
      ]

      const csvContent = csvRows.join('\n')
      const blob = new Blob([csvContent], { type: 'text/csv' })
      const url = window.URL.createObjectURL(blob)
      const link = document.createElement('a')
      link.href = url
      link.download = `chimera_wallets_${new Date().toISOString().split('T')[0]}.csv`
      document.body.appendChild(link)
      link.click()
      document.body.removeChild(link)
      window.URL.revokeObjectURL(url)
      
      toast.success('Wallets exported to CSV')
    } catch (error) {
      console.error('Failed to export wallets:', error)
      toast.error('Failed to export wallets. Please try again.')
    }
  }

  const handlePromote = async () => {
    if (!promoteModal) return

    try {
      await updateWallet.mutateAsync({
        address: promoteModal.address,
        status: 'ACTIVE',
        ttl_hours: ttlHours,
        reason: 'Promoted via dashboard',
      })
      setPromoteModal(null)
      setTtlHours(undefined)
    } catch (error) {
      console.error('Failed to promote wallet:', error)
      toast.error('Failed to promote wallet. Please try again.')
    }
  }

  const handleDemote = async () => {
    if (!demoteModal) return

    try {
      await updateWallet.mutateAsync({
        address: demoteModal.address,
        status: 'CANDIDATE',
        reason: 'Demoted via dashboard',
      })
      setDemoteModal(null)
    } catch (error) {
      console.error('Failed to demote wallet:', error)
      toast.error('Failed to demote wallet. Please try again.')
    }
  }

  return (
    <div className="space-y-6 overflow-x-hidden">
      {/* Header with filters - Mobile Optimized */}
      <div className="space-y-3 md:space-y-4">
        <div className="flex flex-col md:flex-row items-stretch md:items-center justify-between gap-3 md:gap-4">
          <div className="flex flex-col sm:flex-row items-stretch sm:items-center gap-2 md:gap-4">
            {/* Status Filter - Mobile Stacked */}
            <div className="flex rounded-lg border border-border overflow-hidden">
              {(['ALL', 'ACTIVE', 'CANDIDATE', 'REJECTED'] as WalletStatus[]).map((status) => (
                <button
                  key={status}
                  onClick={() => setStatusFilter(status)}
                  className={`px-2 md:px-4 py-1.5 md:py-2 text-xs md:text-sm font-medium transition-colors flex-1 md:flex-none ${
                    statusFilter === status
                      ? 'bg-shield text-background'
                      : 'bg-surface text-text-muted hover:text-text hover:bg-surface-light'
                  }`}
                >
                  {status}
                </button>
              ))}
            </div>

            {/* Search - Full width on mobile */}
            <div className="relative w-full sm:w-auto">
              <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-text-muted" />
              <input
                type="text"
                placeholder="Search by address..."
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
                className="pl-10 pr-4 py-2 bg-surface border border-border rounded-lg text-xs md:text-sm text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-shield focus:border-transparent w-full sm:w-64"
              />
            </div>

            {/* Advanced Filters Toggle */}
            <Button
              variant="secondary"
              size="sm"
              onClick={() => {
                // Toggle advanced filters visibility (could use a state for this)
                const filtersPanel = document.getElementById('advanced-filters')
                if (filtersPanel) {
                  filtersPanel.classList.toggle('hidden')
                }
              }}
            >
              Advanced Filters
            </Button>
          </div>

          {/* Action Buttons - Stack on mobile */}
          <div className="flex flex-col sm:flex-row items-stretch sm:items-center gap-2 w-full sm:w-auto">
            {canModify && selectedWallets.size > 0 && (
              <>
                <Button
                  variant="shield"
                  size="sm"
                  onClick={handleBulkPromote}
                  className="w-full sm:w-auto"
                >
                  <span className="hidden sm:inline">Promote Selected </span>
                  ({selectedWallets.size})
                </Button>
                <Button
                  variant="secondary"
                  size="sm"
                  onClick={handleBulkDemote}
                  className="w-full sm:w-auto"
                >
                  <span className="hidden sm:inline">Demote Selected </span>
                  ({selectedWallets.size})
                </Button>
              </>
            )}
            <Button
              variant="secondary"
              size="sm"
              onClick={handleExportCSV}
              className="w-full sm:w-auto"
            >
              <Download className="w-4 h-4 mr-2" />
              <span className="hidden sm:inline">Export </span>CSV
            </Button>
          </div>
        </div>

        {/* Advanced Filters Panel */}
        <div id="advanced-filters" className="hidden bg-surface-light rounded-lg p-4 space-y-3">
          <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
            <div>
              <label className="block text-sm text-text-muted mb-1">WQS Min</label>
              <input
                type="number"
                min="0"
                max="100"
                placeholder="0"
                value={wqsMinFilter ?? ''}
                onChange={(e) => setWqsMinFilter(e.target.value ? parseFloat(e.target.value) : undefined)}
                className="w-full px-3 py-2 bg-surface border border-border rounded-lg text-sm"
              />
            </div>
            <div>
              <label className="block text-sm text-text-muted mb-1">WQS Max</label>
              <input
                type="number"
                min="0"
                max="100"
                placeholder="100"
                value={wqsMaxFilter ?? ''}
                onChange={(e) => setWqsMaxFilter(e.target.value ? parseFloat(e.target.value) : undefined)}
                className="w-full px-3 py-2 bg-surface border border-border rounded-lg text-sm"
              />
            </div>
            <div>
              <label className="block text-sm text-text-muted mb-1">ROI 30d Min (%)</label>
              <input
                type="number"
                placeholder="0"
                value={roiMinFilter ?? ''}
                onChange={(e) => setRoiMinFilter(e.target.value ? parseFloat(e.target.value) : undefined)}
                className="w-full px-3 py-2 bg-surface border border-border rounded-lg text-sm"
              />
            </div>
            <div>
              <label className="block text-sm text-text-muted mb-1">Trade Count Min</label>
              <input
                type="number"
                min="0"
                placeholder="0"
                value={tradeCountMinFilter ?? ''}
                onChange={(e) => setTradeCountMinFilter(e.target.value ? parseInt(e.target.value) : undefined)}
                className="w-full px-3 py-2 bg-surface border border-border rounded-lg text-sm"
              />
            </div>
          </div>
          <div className="flex gap-2">
            <Button
              variant="secondary"
              size="sm"
              onClick={() => {
                setWqsMinFilter(undefined)
                setWqsMaxFilter(undefined)
                setRoiMinFilter(undefined)
                setTradeCountMinFilter(undefined)
              }}
            >
              Clear Filters
            </Button>
          </div>
        </div>
      </div>

        {/* Stats */}
        <div className="flex items-center gap-6 text-sm">
          <div>
            <span className="text-text-muted">Total:</span>{' '}
            <span className="font-mono-numbers">{wallets.length}</span>
          </div>
          <div>
            <span className="text-text-muted">Active:</span>{' '}
            <span className="font-mono-numbers text-profit">
              {wallets.filter((w) => w.status === 'ACTIVE').length}
            </span>
          </div>
          <div>
            <span className="text-text-muted">Candidates:</span>{' '}
            <span className="font-mono-numbers text-shield">
              {wallets.filter((w) => w.status === 'CANDIDATE').length}
            </span>
          </div>
        </div>

      {/* Wallets Table */}
      <Card padding="none" className="overflow-x-hidden">
        {isLoading ? (
          <div className="p-8 text-center text-text-muted">Loading wallets...</div>
        ) : filteredWallets.length === 0 ? (
          <div className="p-8 text-center text-text-muted">No wallets found</div>
        ) : (
          <div className="w-full overflow-x-auto">
            <Table className="w-full">
              <TableHeader>
                <TableRow hoverable={false}>
                  {canModify && (
                    <TableHead>
                      <button
                        onClick={toggleAllWallets}
                        className="p-1 hover:bg-surface-light rounded"
                      >
                        {selectedWallets.size === filteredWallets.length ? (
                          <CheckSquare className="w-4 h-4" />
                        ) : (
                          <Square className="w-4 h-4" />
                        )}
                      </button>
                    </TableHead>
                  )}
                  <TableHead>Address</TableHead>
                  <TableHead sortable>WQS</TableHead>
                  <TableHead sortable className="hidden md:table-cell">ROI 30d</TableHead>
                  <TableHead sortable className="hidden sm:table-cell">Trades</TableHead>
                  <TableHead sortable className="hidden lg:table-cell">Win Rate</TableHead>
                  <TableHead>Status</TableHead>
                  <TableHead className="hidden xl:table-cell">TTL</TableHead>
                  {canModify && <TableHead className="hidden lg:table-cell">Actions</TableHead>}
                  <TableHead className="w-8"></TableHead>
                </TableRow>
              </TableHeader>
            <TableBody>
              {filteredWallets.map((wallet) => (
                <>
                  <TableRow
                    key={wallet.address}
                    onClick={() =>
                      setExpandedWallet(
                        expandedWallet === wallet.address ? null : wallet.address
                      )
                    }
                  >
                    {canModify && (
                      <TableCell>
                        <button
                          onClick={(e) => {
                            e.stopPropagation()
                            toggleWalletSelection(wallet.address)
                          }}
                          className="p-1 hover:bg-surface-light rounded"
                        >
                          {selectedWallets.has(wallet.address) ? (
                            <CheckSquare className="w-4 h-4" />
                          ) : (
                            <Square className="w-4 h-4" />
                          )}
                        </button>
                      </TableCell>
                    )}
                    <TableCell>
                      <div className="flex items-center gap-2">
                        <span className="font-mono text-xs sm:text-sm">
                          <span className="sm:hidden">{wallet.address.slice(0, 4)}...{wallet.address.slice(-4)}</span>
                          <span className="hidden sm:inline">{wallet.address.slice(0, 8)}...{wallet.address.slice(-4)}</span>
                        </span>
                        <div className="flex items-center gap-1 ml-1">
                          <a
                            href={getWalletSolscanUrl(wallet.address)}
                            target="_blank"
                            rel="noopener noreferrer"
                            onClick={(e) => e.stopPropagation()}
                            className="inline-flex items-center justify-center w-4 h-4 sm:w-5 sm:h-5 text-shield hover:text-shield-dark transition-colors hover:scale-110"
                            title="View wallet on Solscan"
                          >
                            <ExternalLink className="w-3 h-3 sm:w-4 sm:h-4" />
                          </a>
                          <a
                            href={getWalletExplorerUrl(wallet.address)}
                            target="_blank"
                            rel="noopener noreferrer"
                            onClick={(e) => e.stopPropagation()}
                            className="inline-flex items-center justify-center w-4 h-4 sm:w-5 sm:h-5 text-text-muted hover:text-text transition-colors hover:scale-110"
                            title="View wallet on Solana Explorer"
                          >
                            <ExternalLink className="w-3 h-3 sm:w-4 sm:h-4" />
                          </a>
                        </div>
                      </div>
                    </TableCell>
                    <TableCell mono>
                      <div className="flex flex-col">
                        <span
                          className={`text-xs sm:text-sm font-semibold ${
                            (wallet.wqs_score || 0) >= 70
                              ? 'text-profit'
                              : (wallet.wqs_score || 0) >= 40
                              ? 'text-spear'
                              : 'text-loss'
                          }`}
                        >
                          {wallet.wqs_score?.toFixed(1) || '-'}
                        </span>
                        {wallet.wqs_score !== null && (
                          <div className="h-1.5 bg-background rounded-full overflow-hidden mt-1 w-full sm:max-w-[60px]">
                            <div
                              className={`h-full ${
                                wallet.wqs_score >= 70
                                  ? 'bg-profit'
                                  : wallet.wqs_score >= 40
                                  ? 'bg-spear'
                                  : 'bg-loss'
                              }`}
                              style={{ width: `${Math.min(wallet.wqs_score, 100)}%` }}
                            />
                          </div>
                        )}
                      </div>
                    </TableCell>
                    <TableCell mono className="hidden md:table-cell">
                      {wallet.roi_30d !== null ? (
                        <span
                          className={wallet.roi_30d >= 0 ? 'text-profit' : 'text-loss'}
                        >
                          {wallet.roi_30d >= 0 ? '+' : ''}
                          {wallet.roi_30d.toFixed(1)}%
                        </span>
                      ) : (
                        '-'
                      )}
                    </TableCell>
                    <TableCell mono className="hidden sm:table-cell">{wallet.trade_count_30d || '-'}</TableCell>
                    <TableCell mono className="hidden lg:table-cell">
                      {wallet.win_rate !== null
                        ? `${(wallet.win_rate * 100).toFixed(0)}%`
                        : '-'}
                    </TableCell>
                    <TableCell>
                      <StatusBadge status={wallet.status} />
                    </TableCell>
                    <TableCell className="hidden xl:table-cell">
                      {wallet.ttl_expires_at ? (
                        <Badge variant="warning" size="sm">
                          {formatTTL(wallet.ttl_expires_at)}
                        </Badge>
                      ) : (
                        '-'
                      )}
                    </TableCell>
                    {canModify && (
                      <TableCell className="hidden lg:table-cell">
                        <div className="flex gap-2">
                          {wallet.status === 'CANDIDATE' && (
                            <Button
                              variant="shield"
                              size="sm"
                              onClick={(e) => {
                                e.stopPropagation()
                                setPromoteModal(wallet)
                              }}
                            >
                              Promote
                            </Button>
                          )}
                          {wallet.status === 'ACTIVE' && (
                            <Button
                              variant="secondary"
                              size="sm"
                              onClick={(e) => {
                                e.stopPropagation()
                                setDemoteModal(wallet)
                              }}
                            >
                              Demote
                            </Button>
                          )}
                        </div>
                      </TableCell>
                    )}
                    <TableCell>
                      {expandedWallet === wallet.address ? (
                        <ChevronUp className="w-4 h-4 text-text-muted" />
                      ) : (
                        <ChevronDown className="w-4 h-4 text-text-muted" />
                      )}
                    </TableCell>
                  </TableRow>

                  {/* Expanded Row */}
                  {expandedWallet === wallet.address && (
                    <tr className="bg-surface-light">
                      <td colSpan={canModify ? 10 : 9} className="px-2 sm:px-4 py-4 w-full">
                        <div className="w-full max-w-full overflow-x-hidden">
                          <WalletDetails wallet={wallet} />
                        </div>
                      </td>
                    </tr>
                  )}
                </>
              ))}
            </TableBody>
          </Table>
          </div>
        )}
      </Card>

      {/* Promote Modal */}
      <Modal
        isOpen={!!promoteModal}
        onClose={() => {
          setPromoteModal(null)
          setTtlHours(undefined)
        }}
        title="Promote Wallet"
        size="sm"
      >
        {promoteModal && (
          <div className="space-y-4">
            <p className="text-text-muted">
              Promote <code className="text-shield">{promoteModal.address.slice(0, 16)}...</code> to
              ACTIVE status?
            </p>

            <div>
              <label className="block text-sm font-medium text-text mb-2">
                TTL (optional)
              </label>
              <select
                value={ttlHours || ''}
                onChange={(e) =>
                  setTtlHours(e.target.value ? parseInt(e.target.value) : undefined)
                }
                className="w-full bg-surface border border-border rounded-lg px-3 py-2 text-text focus:outline-none focus:ring-2 focus:ring-shield"
              >
                <option value="">Permanent</option>
                <option value="24">24 hours</option>
                <option value="48">48 hours</option>
                <option value="72">72 hours</option>
                <option value="168">1 week</option>
              </select>
              <p className="text-xs text-text-muted mt-1">
                Wallet will auto-demote after TTL expires
              </p>
            </div>

            <div className="flex gap-3 justify-end">
              <Button
                variant="secondary"
                onClick={() => {
                  setPromoteModal(null)
                  setTtlHours(undefined)
                }}
              >
                Cancel
              </Button>
              <Button
                variant="shield"
                onClick={handlePromote}
                loading={updateWallet.isPending}
              >
                Promote
              </Button>
            </div>
          </div>
        )}
      </Modal>

      {/* Demote Confirmation */}
      <ConfirmModal
        isOpen={!!demoteModal}
        onClose={() => setDemoteModal(null)}
        onConfirm={handleDemote}
        title="Demote Wallet"
        message={`Are you sure you want to demote ${demoteModal?.address.slice(0, 16)}... to CANDIDATE status?`}
        confirmLabel="Demote"
        variant="warning"
        loading={updateWallet.isPending}
      />
    </div>
  )
}

function WalletDetails({ wallet }: { wallet: Wallet }) {
  // Fetch trade history for this wallet
  const { data: tradesData, isLoading: tradesLoading } = useTrades({
    wallet_address: wallet.address,
    limit: 20, // Show last 20 trades
  })

  const walletTrades = tradesData?.trades || []

  return (
    <div className="space-y-4 sm:space-y-6 w-full max-w-full overflow-x-hidden">
      {/* Wallet Address with Explorer Links */}
      <div className="pb-3 sm:pb-4 border-b border-border">
        <div className="flex flex-col sm:flex-row sm:items-center gap-2 sm:gap-3">
          <div className="flex-1 min-w-0">
            <h3 className="text-xs sm:text-sm font-semibold text-text-muted uppercase mb-1">Wallet Address</h3>
            <div className="flex items-center gap-2 flex-wrap">
              <span className="font-mono text-xs sm:text-sm break-all">{wallet.address}</span>
              <div className="flex items-center gap-2 flex-shrink-0">
                <a
                  href={getWalletSolscanUrl(wallet.address)}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="inline-flex items-center justify-center w-5 h-5 text-shield hover:text-shield-dark transition-colors hover:scale-110"
                  title="View wallet on Solscan"
                >
                  <ExternalLink className="w-4 h-4" />
                </a>
                <a
                  href={getWalletExplorerUrl(wallet.address)}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="inline-flex items-center justify-center w-5 h-5 text-text-muted hover:text-text transition-colors hover:scale-110"
                  title="View wallet on Solana Explorer"
                >
                  <ExternalLink className="w-4 h-4" />
                </a>
              </div>
            </div>
          </div>
        </div>
      </div>

      {/* Performance Metrics Grid */}
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4 sm:gap-6">
        <div className="bg-surface-light rounded-lg p-3 sm:p-4">
          <h4 className="text-xs font-semibold text-text-muted uppercase mb-2 sm:mb-3">
            Performance
          </h4>
          <div className="space-y-1.5 sm:space-y-2 text-xs sm:text-sm">
            <div className="flex justify-between items-center">
              <span className="text-text-muted">ROI 7d:</span>
              <span className={`font-mono-numbers text-xs sm:text-sm ${
                wallet.roi_7d !== null && wallet.roi_7d >= 0 ? 'text-profit' : 'text-loss'
              }`}>
                {wallet.roi_7d !== null
                  ? `${wallet.roi_7d >= 0 ? '+' : ''}${wallet.roi_7d.toFixed(1)}%`
                  : '-'}
              </span>
            </div>
            <div className="flex justify-between items-center">
              <span className="text-text-muted">ROI 30d:</span>
              <span className={`font-mono-numbers text-xs sm:text-sm ${
                wallet.roi_30d !== null && wallet.roi_30d >= 0 ? 'text-profit' : 'text-loss'
              }`}>
                {wallet.roi_30d !== null
                  ? `${wallet.roi_30d >= 0 ? '+' : ''}${wallet.roi_30d.toFixed(1)}%`
                  : '-'}
              </span>
            </div>
            <div className="flex justify-between items-center">
              <span className="text-text-muted">Max Drawdown:</span>
              <span className="font-mono-numbers text-xs sm:text-sm text-loss">
                {wallet.max_drawdown_30d !== null
                  ? `-${wallet.max_drawdown_30d.toFixed(1)}%`
                  : '-'}
              </span>
            </div>
            <div className="flex justify-between items-center">
              <span className="text-text-muted">Avg Trade:</span>
              <span className="font-mono-numbers text-xs sm:text-sm">
                {wallet.avg_trade_size_sol !== null
                  ? `${wallet.avg_trade_size_sol.toFixed(3)} SOL`
                  : '-'}
              </span>
            </div>
          </div>
        </div>

        <div className="bg-surface-light rounded-lg p-3 sm:p-4">
          <h4 className="text-xs font-semibold text-text-muted uppercase mb-2 sm:mb-3">
            Activity
          </h4>
          <div className="space-y-1.5 sm:space-y-2 text-xs sm:text-sm">
            <div className="flex justify-between items-center">
              <span className="text-text-muted">Trade Count:</span>
              <span className="font-mono-numbers text-xs sm:text-sm">
                {wallet.trade_count_30d || 0}
              </span>
            </div>
            <div className="flex justify-between items-center">
              <span className="text-text-muted">Win Rate:</span>
              <span className="font-mono-numbers text-xs sm:text-sm">
                {wallet.win_rate !== null
                  ? `${(wallet.win_rate * 100).toFixed(1)}%`
                  : '-'}
              </span>
            </div>
            <div className="flex justify-between items-center">
              <span className="text-text-muted">Last Trade:</span>
              <span className="text-xs sm:text-sm">{wallet.last_trade_at ? formatDate(wallet.last_trade_at) : '-'}</span>
            </div>
            <div className="flex justify-between items-center">
              <span className="text-text-muted">Created:</span>
              <span className="text-xs sm:text-sm">{formatDate(wallet.created_at)}</span>
            </div>
          </div>
        </div>

        <div className="bg-surface-light rounded-lg p-3 sm:p-4">
          <h4 className="text-xs font-semibold text-text-muted uppercase mb-2 sm:mb-3">
            Promotion
          </h4>
          <div className="space-y-1.5 sm:space-y-2 text-xs sm:text-sm">
            <div className="flex justify-between items-center">
              <span className="text-text-muted">Status:</span>
              <StatusBadge status={wallet.status} />
            </div>
            <div className="flex justify-between items-center">
              <span className="text-text-muted">Promoted:</span>
              <span className="text-xs sm:text-sm">{wallet.promoted_at ? formatDate(wallet.promoted_at) : '-'}</span>
            </div>
            <div className="flex justify-between items-center">
              <span className="text-text-muted">TTL Expires:</span>
              <span className="text-xs sm:text-sm">
                {wallet.ttl_expires_at ? formatDate(wallet.ttl_expires_at) : 'Never'}
              </span>
            </div>
            <div className="flex justify-between items-center">
              <span className="text-text-muted">Updated:</span>
              <span className="text-xs sm:text-sm">{formatDate(wallet.updated_at)}</span>
            </div>
          </div>
        </div>

        <div className="bg-surface-light rounded-lg p-3 sm:p-4">
          <h4 className="text-xs font-semibold text-text-muted uppercase mb-2 sm:mb-3">
            WQS Breakdown
          </h4>
          <div className="space-y-2 text-xs sm:text-sm">
            <div className="flex justify-between items-center">
              <span className="text-text-muted">Score:</span>
              <span className={`font-mono-numbers font-semibold text-xs sm:text-sm ${
                (wallet.wqs_score || 0) >= 70
                  ? 'text-profit'
                  : (wallet.wqs_score || 0) >= 40
                  ? 'text-spear'
                  : 'text-loss'
              }`}>
                {wallet.wqs_score?.toFixed(1) || '-'}
              </span>
            </div>
            {wallet.wqs_score !== null && (
              <div className="mt-2">
                <div className="h-1.5 bg-background rounded-full overflow-hidden w-full sm:max-w-[120px]">
                  <div
                    className={`h-full ${
                      wallet.wqs_score >= 70
                        ? 'bg-profit'
                        : wallet.wqs_score >= 40
                        ? 'bg-spear'
                        : 'bg-loss'
                    }`}
                    style={{ width: `${Math.min(wallet.wqs_score, 100)}%` }}
                  />
                </div>
              </div>
            )}
            <div className="mt-2 text-xs text-text-muted break-words">
              {wallet.notes || 'No notes'}
            </div>
          </div>
        </div>
      </div>

      {/* Trade History */}
      <div>
        <h4 className="text-xs font-semibold text-text-muted uppercase mb-2 sm:mb-3">
          Recent Trade History
        </h4>
        {tradesLoading ? (
          <div className="text-xs sm:text-sm text-text-muted">Loading trades...</div>
        ) : walletTrades.length === 0 ? (
          <div className="text-xs sm:text-sm text-text-muted">No trades found for this wallet</div>
        ) : (
          <div className="overflow-x-auto -mx-4 sm:mx-0 w-full">
            <div className="inline-block min-w-full align-middle px-4 sm:px-0 w-full">
              <Table className="w-full">
                <TableHeader>
                  <TableRow hoverable={false}>
                    <TableHead className="text-xs">Date</TableHead>
                    <TableHead className="text-xs">Token</TableHead>
                    <TableHead className="text-xs hidden sm:table-cell">Strategy</TableHead>
                    <TableHead className="text-xs">Side</TableHead>
                    <TableHead className="text-xs">Amount</TableHead>
                    <TableHead className="text-xs hidden md:table-cell">Status</TableHead>
                    <TableHead className="text-xs">PnL</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {walletTrades.slice(0, 10).map((trade) => (
                    <TableRow key={trade.trade_uuid}>
                      <TableCell className="text-xs">
                        <div className="whitespace-nowrap">
                          {formatDate(trade.created_at)}
                        </div>
                      </TableCell>
                      <TableCell>
                        <div className="font-semibold text-xs sm:text-sm">
                          {trade.token_symbol || 'Unknown'}
                        </div>
                      </TableCell>
                      <TableCell className="hidden sm:table-cell">
                        <Badge variant={trade.strategy === 'SHIELD' ? 'shield' : 'spear'} size="sm">
                          {trade.strategy}
                        </Badge>
                      </TableCell>
                      <TableCell>
                        <span className={`text-xs sm:text-sm ${
                          trade.side === 'BUY' ? 'text-profit' : 'text-loss'
                        }`}>
                          {trade.side}
                        </span>
                      </TableCell>
                      <TableCell mono className="text-xs">
                        {trade.amount_sol.toFixed(4)} SOL
                      </TableCell>
                      <TableCell className="hidden md:table-cell">
                        <StatusBadge status={trade.status} />
                      </TableCell>
                      <TableCell mono className="text-xs">
                        {trade.pnl_usd !== null ? (
                          <span className={trade.pnl_usd >= 0 ? 'text-profit' : 'text-loss'}>
                            {trade.pnl_usd >= 0 ? '+' : ''}${trade.pnl_usd.toFixed(2)}
                          </span>
                        ) : (
                          '-'
                        )}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          </div>
        )}
      </div>

      {/* Backtest Results Placeholder */}
      <div>
        <h4 className="text-xs font-semibold text-text-muted uppercase mb-2 sm:mb-3">
          Backtest Results
        </h4>
        <div className="bg-surface-light rounded-lg p-3 sm:p-4 text-xs sm:text-sm text-text-muted">
          <p>Backtest results are generated during wallet promotion validation.</p>
          <p className="mt-2 text-xs">
            Results include: simulated PnL, win rate, max drawdown, and liquidity checks.
          </p>
        </div>
      </div>
    </div>
  )
}

function formatTTL(dateStr: string): string {
  const date = new Date(dateStr)
  const now = new Date()
  const diffMs = date.getTime() - now.getTime()

  if (diffMs < 0) return 'Expired'

  const hours = Math.floor(diffMs / (1000 * 60 * 60))
  if (hours < 24) return `${hours}h left`

  const days = Math.floor(hours / 24)
  return `${days}d left`
}

function formatDate(dateStr: string): string {
  const date = new Date(dateStr)
  return date.toLocaleDateString('en-US', {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}
