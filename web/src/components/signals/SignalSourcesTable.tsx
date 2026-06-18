import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../ui/Table'
import { Badge } from '../ui/Badge'
import type { SignalSource } from '../../api'

interface SignalSourcesTableProps {
  sources: SignalSource[]
}

export function SignalSourcesTable({ sources }: SignalSourcesTableProps) {
  // Sort by signal count
  const sortedSources = [...sources].sort((a, b) => b.signal_count - a.signal_count)

  return (
    <Table>
      <TableHeader>
        <TableRow hoverable={false}>
          <TableHead>Source</TableHead>
          <TableHead className="text-right">Signals</TableHead>
          <TableHead className="text-right">Avg Quality</TableHead>
          <TableHead className="text-right">Acceptance</TableHead>
          <TableHead>Last Signal</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {sortedSources.map((source) => (
          <TableRow key={source.source}>
            <TableCell mono className="text-sm">
              {source.source.slice(0, 8)}...{source.source.slice(-8)}
            </TableCell>
            <TableCell mono className="text-sm text-right">
              {source.signal_count}
            </TableCell>
            <TableCell mono className="text-sm text-right">
              <Badge
                variant={source.average_quality >= 0.7 ? 'success' : source.average_quality >= 0.5 ? 'warning' : 'default'}
                size="sm"
              >
                {source.average_quality.toFixed(2)}
              </Badge>
            </TableCell>
            <TableCell className="text-sm text-right">
              <span className={source.acceptance_rate >= 0.7 ? 'text-profit' : source.acceptance_rate >= 0.4 ? 'text-spear' : 'text-loss'}>
                {(source.acceptance_rate * 100).toFixed(1)}%
              </span>
            </TableCell>
            <TableCell className="text-sm text-text-muted">
              {source.last_signal_at
                ? new Date(source.last_signal_at).toLocaleString()
                : 'Never'}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  )
}
