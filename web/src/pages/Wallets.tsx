import { useState } from 'react'
import { Search, ChevronDown, ChevronUp } from 'lucide-react'
import { Card } from '../components/ui/Card'
import { Button } from '../components/ui/Button'
import { Badge, StatusBadge } from '../components/ui/Badge'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../components/ui/Table'
import { Modal, ConfirmModal } from '../components/ui/Modal'
import { useWallets, useUpdateWallet } from '../api'
import { useAuthStore } from '../stores/authStore'
import type { Wallet } from '../types'

type WalletStatus = 'ALL' | 'ACTIVE' | 'CANDIDATE' | 'REJECTED'

export function Wallets() {
  const [statusFilter, setStatusFilter] = useState<WalletStatus>('ALL')
  const [searchQuery, setSearchQuery] = useState('')
  const [expandedWallet, setExpandedWallet] = useState<string | null>(null)
  const [promoteModal, setPromoteModal] = useState<Wallet | null>(null)
  const [demoteModal, setDemoteModal] = useState<Wallet | null>(null)
  const [ttlHours, setTtlHours] = useState<number | undefined>(undefined)

  const { hasPermission } = useAuthStore()
  const canModify = hasPermission('operator')

  const { data: walletsData, isLoading } = useWallets(
    statusFilter === 'ALL' ? undefined : statusFilter
  )
  const updateWallet = useUpdateWallet()

  const wallets = walletsData?.wallets || []

  // Filter by search query
  const filteredWallets = wallets.filter((wallet) =>
    wallet.address.toLowerCase().includes(searchQuery.toLowerCase())
  )

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
    }
  }

  return (
    <div className="space-y-6">
      {/* Header with filters */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-4">
          {/* Status Filter */}
          <div className="flex rounded-lg border border-border overflow-hidden">
            {(['ALL', 'ACTIVE', 'CANDIDATE', 'REJECTED'] as WalletStatus[]).map((status) => (
              <button
                key={status}
                onClick={() => setStatusFilter(status)}
                className={`px-4 py-2 text-sm font-medium transition-colors ${
                  statusFilter === status
                    ? 'bg-shield text-background'
                    : 'bg-surface text-text-muted hover:text-text hover:bg-surface-light'
                }`}
              >
                {status}
              </button>
            ))}
          </div>

          {/* Search */}
          <div className="relative">
            <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-text-muted" />
            <input
              type="text"
              placeholder="Search by address..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="pl-10 pr-4 py-2 bg-surface border border-border rounded-lg text-sm text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-shield focus:border-transparent w-64"
            />
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
      </div>

      {/* Wallets Table */}
      <Card padding="none">
        {isLoading ? (
          <div className="p-8 text-center text-text-muted">Loading wallets...</div>
        ) : filteredWallets.length === 0 ? (
          <div className="p-8 text-center text-text-muted">No wallets found</div>
        ) : (
          <Table>
            <TableHeader>
              <TableRow hoverable={false}>
                <TableHead>Address</TableHead>
                <TableHead sortable>WQS</TableHead>
                <TableHead sortable>ROI 30d</TableHead>
                <TableHead sortable>Trades</TableHead>
                <TableHead sortable>Win Rate</TableHead>
                <TableHead>Status</TableHead>
                <TableHead>TTL</TableHead>
                {canModify && <TableHead>Actions</TableHead>}
                <TableHead></TableHead>
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
                    <TableCell>
                      <div className="font-mono text-sm">
                        {wallet.address.slice(0, 8)}...{wallet.address.slice(-4)}
                      </div>
                    </TableCell>
                    <TableCell mono>
                      <span
                        className={
                          (wallet.wqs_score || 0) >= 70
                            ? 'text-profit'
                            : (wallet.wqs_score || 0) >= 40
                            ? 'text-spear'
                            : 'text-loss'
                        }
                      >
                        {wallet.wqs_score?.toFixed(1) || '-'}
                      </span>
                    </TableCell>
                    <TableCell mono>
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
                    <TableCell mono>{wallet.trade_count_30d || '-'}</TableCell>
                    <TableCell mono>
                      {wallet.win_rate !== null
                        ? `${(wallet.win_rate * 100).toFixed(0)}%`
                        : '-'}
                    </TableCell>
                    <TableCell>
                      <StatusBadge status={wallet.status} />
                    </TableCell>
                    <TableCell>
                      {wallet.ttl_expires_at ? (
                        <Badge variant="warning" size="sm">
                          {formatTTL(wallet.ttl_expires_at)}
                        </Badge>
                      ) : (
                        '-'
                      )}
                    </TableCell>
                    {canModify && (
                      <TableCell>
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
                      <td colSpan={canModify ? 9 : 8} className="px-4 py-4">
                        <WalletDetails wallet={wallet} />
                      </td>
                    </tr>
                  )}
                </>
              ))}
            </TableBody>
          </Table>
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
  return (
    <div className="grid grid-cols-4 gap-6">
      <div>
        <h4 className="text-xs font-semibold text-text-muted uppercase mb-2">
          Performance
        </h4>
        <div className="space-y-1 text-sm">
          <div className="flex justify-between">
            <span className="text-text-muted">ROI 7d:</span>
            <span className="font-mono-numbers">
              {wallet.roi_7d !== null
                ? `${wallet.roi_7d >= 0 ? '+' : ''}${wallet.roi_7d.toFixed(1)}%`
                : '-'}
            </span>
          </div>
          <div className="flex justify-between">
            <span className="text-text-muted">Max Drawdown:</span>
            <span className="font-mono-numbers text-loss">
              {wallet.max_drawdown_30d !== null
                ? `-${wallet.max_drawdown_30d.toFixed(1)}%`
                : '-'}
            </span>
          </div>
          <div className="flex justify-between">
            <span className="text-text-muted">Avg Trade:</span>
            <span className="font-mono-numbers">
              {wallet.avg_trade_size_sol !== null
                ? `${wallet.avg_trade_size_sol.toFixed(3)} SOL`
                : '-'}
            </span>
          </div>
        </div>
      </div>

      <div>
        <h4 className="text-xs font-semibold text-text-muted uppercase mb-2">
          Activity
        </h4>
        <div className="space-y-1 text-sm">
          <div className="flex justify-between">
            <span className="text-text-muted">Last Trade:</span>
            <span>{wallet.last_trade_at ? formatDate(wallet.last_trade_at) : '-'}</span>
          </div>
          <div className="flex justify-between">
            <span className="text-text-muted">Created:</span>
            <span>{formatDate(wallet.created_at)}</span>
          </div>
          <div className="flex justify-between">
            <span className="text-text-muted">Updated:</span>
            <span>{formatDate(wallet.updated_at)}</span>
          </div>
        </div>
      </div>

      <div>
        <h4 className="text-xs font-semibold text-text-muted uppercase mb-2">
          Promotion
        </h4>
        <div className="space-y-1 text-sm">
          <div className="flex justify-between">
            <span className="text-text-muted">Promoted:</span>
            <span>{wallet.promoted_at ? formatDate(wallet.promoted_at) : '-'}</span>
          </div>
          <div className="flex justify-between">
            <span className="text-text-muted">TTL Expires:</span>
            <span>
              {wallet.ttl_expires_at ? formatDate(wallet.ttl_expires_at) : 'Never'}
            </span>
          </div>
        </div>
      </div>

      <div>
        <h4 className="text-xs font-semibold text-text-muted uppercase mb-2">
          Notes
        </h4>
        <p className="text-sm text-text-muted">
          {wallet.notes || 'No notes'}
        </p>
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
