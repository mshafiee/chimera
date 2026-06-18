import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../ui/Table'
import { Badge } from '../ui/Badge'
import type { ReconciliationRun } from '../../api'

interface ReconciliationHistoryProps {
  runs: ReconciliationRun[]
}

export function ReconciliationHistory({ runs }: ReconciliationHistoryProps) {
  const statusVariant = {
    completed: 'success',
    failed: 'danger',
    running: 'warning',
    pending: 'default',
  } as const

  return (
    <Table>
      <TableHeader>
        <TableRow hoverable={false}>
          <TableHead>Started</TableHead>
          <TableHead>Status</TableHead>
          <TableHead className="text-right">Checked</TableHead>
          <TableHead className="text-right">Discrepancies</TableHead>
          <TableHead className="text-right">Unresolved</TableHead>
          <TableHead className="text-right">Duration</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {runs.map((run) => (
          <TableRow key={run.id}>
            <TableCell className="text-sm">
              {new Date(run.started_at).toLocaleString()}
            </TableCell>
            <TableCell>
              <Badge variant={statusVariant[run.status]} size="sm">
                {run.status}
              </Badge>
            </TableCell>
            <TableCell mono className="text-sm text-right">
              {run.checked_count}
            </TableCell>
            <TableCell mono className="text-sm text-right">
              {run.discrepancy_count}
            </TableCell>
            <TableCell mono className="text-sm text-right">
              <span className={run.unresolved_count > 0 ? 'text-loss' : 'text-profit'}>
                {run.unresolved_count}
              </span>
            </TableCell>
            <TableCell mono className="text-sm text-right">
              {run.duration_seconds ? `${run.duration_seconds.toFixed(1)}s` : '—'}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  )
}
