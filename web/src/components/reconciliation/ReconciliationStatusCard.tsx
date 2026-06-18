import { Card, CardHeader, CardTitle, CardContent } from '../ui/Card'
import { Badge } from '../ui/Badge'
import { Clock, CheckCircle, XCircle, Loader } from 'lucide-react'
import type { ReconciliationStatusResponse } from '../../api'

interface ReconciliationStatusCardProps {
  status: ReconciliationStatusResponse | null | undefined
  isLoading?: boolean
}

export function ReconciliationStatusCard({ status, isLoading }: ReconciliationStatusCardProps) {
  if (isLoading) {
    return (
      <Card>
        <CardContent className="p-6">
          <div className="text-center text-text-muted">Loading reconciliation status...</div>
        </CardContent>
      </Card>
    )
  }

  if (!status) {
    return (
      <Card>
        <CardContent className="p-6">
          <div className="text-center text-text-muted">No reconciliation status available</div>
        </CardContent>
      </Card>
    )
  }

  const statusConfig = {
    pending: { icon: Clock, color: 'text-text-muted', label: 'Pending' },
    running: { icon: Loader, color: 'text-spear', label: 'Running' },
    completed: { icon: CheckCircle, color: 'text-profit', label: 'Completed' },
    failed: { icon: XCircle, color: 'text-loss', label: 'Failed' },
  }

  const config = statusConfig[status.status]
  const Icon = config.icon

  return (
    <Card>
      <CardHeader>
        <CardTitle>Reconciliation Status</CardTitle>
      </CardHeader>
      <CardContent>
        <div className="flex items-center gap-6">
          {/* Status */}
          <div className="flex items-center gap-3">
            <Icon className={`w-8 h-8 ${config.color} ${status.status === 'running' ? 'animate-spin' : ''}`} />
            <div>
              <Badge
                variant={
                  status.status === 'completed' ? 'success' :
                  status.status === 'failed' ? 'danger' :
                  status.status === 'running' ? 'warning' : 'default'
                }
                size="md"
              >
                {config.label}
              </Badge>
            </div>
          </div>

          {/* Last Run */}
          {status.last_reconciliation_at && (
            <div className="text-sm">
              <span className="text-text-muted">Last run: </span>
              <span className="font-medium">
                {new Date(status.last_reconciliation_at).toLocaleString()}
              </span>
            </div>
          )}

          {/* Duration */}
          {status.duration_seconds && (
            <div className="text-sm">
              <span className="text-text-muted">Duration: </span>
              <span className="font-mono-numbers font-medium">
                {status.duration_seconds.toFixed(1)}s
              </span>
            </div>
          )}

          {/* Next Run */}
          {status.next_reconciliation_at && (
            <div className="text-sm">
              <span className="text-text-muted">Next: </span>
              <span className="font-medium">
                {new Date(status.next_reconciliation_at).toLocaleString()}
              </span>
            </div>
          )}
        </div>
      </CardContent>
    </Card>
  )
}
