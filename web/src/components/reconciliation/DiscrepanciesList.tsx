import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../ui/Table'
import { Badge } from '../ui/Badge'
import type { Discrepancy } from '../../api'

interface DiscrepanciesListProps {
  discrepancies: Discrepancy[]
}

const TYPE_LABELS: Record<string, string> = {
  missing_position: 'Missing Position',
  pnl_mismatch: 'PnL Mismatch',
  state_mismatch: 'State Mismatch',
  cost_mismatch: 'Cost Mismatch',
}

const SEVERITY_VARIANTS = {
  low: 'default',
  medium: 'warning',
  high: 'danger',
  critical: 'danger',
} as const

export function DiscrepanciesList({ discrepancies }: DiscrepanciesListProps) {
  return (
    <Table>
      <TableHeader>
        <TableRow hoverable={false}>
          <TableHead>Status</TableHead>
          <TableHead>Detected</TableHead>
          <TableHead>Type</TableHead>
          <TableHead>Trade UUID</TableHead>
          <TableHead>DB Value</TableHead>
          <TableHead>On-Chain Value</TableHead>
          <TableHead>Description</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {discrepancies.map((discrepancy) => (
          <TableRow key={discrepancy.id}>
            <TableCell>
              <Badge
                variant={discrepancy.resolved ? 'success' : 'danger'}
                size="sm"
              >
                {discrepancy.resolved ? 'Resolved' : 'Open'}
              </Badge>
            </TableCell>
            <TableCell className="text-sm text-text-muted">
              {new Date(discrepancy.detected_at).toLocaleString()}
            </TableCell>
            <TableCell>
              <Badge variant={SEVERITY_VARIANTS[discrepancy.severity]} size="sm">
                {discrepancy.severity}
              </Badge>
              <div className="text-xs text-text-muted mt-1">
                {TYPE_LABELS[discrepancy.type] || discrepancy.type}
              </div>
            </TableCell>
            <TableCell mono className="text-sm">
              {discrepancy.trade_uuid.slice(0, 8)}...
            </TableCell>
            <TableCell mono className="text-sm text-text-muted">
              {discrepancy.db_value || '—'}
            </TableCell>
            <TableCell mono className="text-sm text-text-muted">
              {discrepancy.on_chain_value || '—'}
            </TableCell>
            <TableCell className="text-sm">
              {discrepancy.description}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  )
}
