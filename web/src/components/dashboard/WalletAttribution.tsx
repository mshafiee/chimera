import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../ui/Table'
import { Badge } from '../ui/Badge'
import { safeToFixed, toNum } from '../../lib/format'
import type { Wallet } from '../../types'

interface WalletAttributionProps {
  wallets: Wallet[]
}

export function WalletAttribution({ wallets }: WalletAttributionProps) {
  // Sort by ROI
  const sortedWallets = [...wallets].sort((a, b) => toNum(b.roi_30d) - toNum(a.roi_30d))

  return (
    <Table>
      <TableHeader>
        <TableRow hoverable={false}>
          <TableHead>Address</TableHead>
          <TableHead className="text-right">WQS</TableHead>
          <TableHead className="text-right">30d ROI</TableHead>
          <TableHead className="text-right">Win Rate</TableHead>
          <TableHead className="text-right">Trades</TableHead>
          <TableHead>Status</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {sortedWallets.map((wallet) => (
          <TableRow key={wallet.address}>
            <TableCell mono className="text-sm">
              {wallet.address.slice(0, 8)}...{wallet.address.slice(-8)}
            </TableCell>
            <TableCell mono className="text-sm text-right">
              <Badge
                variant={wallet.wqs_score && wallet.wqs_score >= 60 ? 'success' : wallet.wqs_score && wallet.wqs_score >= 40 ? 'warning' : 'default'}
                size="sm"
              >
                {wallet.wqs_score?.toFixed(1) || 'N/A'}
              </Badge>
            </TableCell>
            <TableCell mono className="text-sm text-right">
              <span className={wallet.roi_30d && toNum(wallet.roi_30d) >= 0 ? 'text-profit' : 'text-loss'}>
                {wallet.roi_30d !== null && wallet.roi_30d !== undefined ? `${toNum(wallet.roi_30d) >= 0 ? '+' : ''}${safeToFixed(wallet.roi_30d, 1)}%` : 'N/A'}
              </span>
            </TableCell>
            <TableCell mono className="text-sm text-right">
              {wallet.win_rate ? `${wallet.win_rate.toFixed(1)}%` : 'N/A'}
            </TableCell>
            <TableCell mono className="text-sm text-right">
              {wallet.trade_count_30d || 0}
            </TableCell>
            <TableCell>
              <Badge
                variant={wallet.status === 'ACTIVE' ? 'success' : wallet.status === 'CANDIDATE' ? 'warning' : 'default'}
                size="sm"
              >
                {wallet.status}
              </Badge>
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  )
}
