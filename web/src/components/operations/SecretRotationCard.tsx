import { Badge } from '../ui/Badge'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../ui/Table'
import type { SecretRotationResponse } from '../../api'
import { Clock, CheckCircle, XCircle } from 'lucide-react'

interface SecretRotationCardProps {
  data: SecretRotationResponse
}

export function SecretRotationCard({ data }: SecretRotationCardProps) {
  const getStatusVariant = (status: string) => {
    switch (status) {
      case 'active': return 'success'
      case 'due_soon': return 'warning'
      case 'overdue': return 'danger'
      default: return 'default'
    }
  }

  const getStatusConfig = (status: string) => {
    switch (status) {
      case 'active':
        return { icon: CheckCircle, color: 'text-profit', label: 'Active' }
      case 'due_soon':
        return { icon: Clock, color: 'text-spear', label: 'Due Soon' }
      case 'overdue':
        return { icon: XCircle, color: 'text-loss', label: 'Overdue' }
      default:
        return { icon: Clock, color: 'text-text-muted', label: 'Unknown' }
    }
  }

  const statusConfig = getStatusConfig(data.status)
  const StatusIcon = statusConfig.icon

  return (
    <div className="space-y-6">
      {/* Current Status */}
      <div className="flex items-center gap-4">
        <StatusIcon className={`w-8 h-8 ${statusConfig.color}`} />
        <div>
          <div className="text-sm text-text-muted">Rotation Status</div>
          <Badge variant={getStatusVariant(data.status)} size="lg">
            {statusConfig.label}
          </Badge>
        </div>
        {data.days_until_due !== null && (
          <div className="ml-auto">
            <div className="text-sm text-text-muted">Days Until Due</div>
            <div className={`text-xl font-semibold font-mono-numbers ${
              data.days_until_due < 3 ? 'text-loss' : data.days_until_due < 7 ? 'text-spear' : 'text-profit'
            }`}>
              {data.days_until_due}
            </div>
          </div>
        )}
      </div>

      {/* Last/Next Rotation */}
      <div className="grid grid-cols-2 gap-4 pt-4 border-t border-border">
        <div>
          <div className="text-sm text-text-muted">Last Rotation</div>
          <div className="font-medium">
            {data.last_rotation_at
              ? new Date(data.last_rotation_at).toLocaleString()
              : 'Never'}
          </div>
        </div>
        <div>
          <div className="text-sm text-text-muted">Next Rotation</div>
          <div className="font-medium">
            {data.next_rotation_at
              ? new Date(data.next_rotation_at).toLocaleString()
              : 'Not scheduled'}
          </div>
        </div>
      </div>

      {/* Rotation History */}
      {data.rotation_history.length > 0 && (
        <div>
          <h3 className="text-sm font-medium mb-3">Rotation History</h3>
          <Table>
            <TableHeader>
              <TableRow hoverable={false}>
                <TableHead>Timestamp</TableHead>
                <TableHead>Status</TableHead>
                <TableHead className="text-right">Duration</TableHead>
                <TableHead className="text-right">Keys Rotated</TableHead>
                <TableHead className="text-right">Failed</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {data.rotation_history.map((event, index) => (
                <TableRow key={index}>
                  <TableCell className="text-sm">
                    {new Date(event.timestamp).toLocaleString()}
                  </TableCell>
                  <TableCell>
                    <Badge
                      variant={event.status === 'success' ? 'success' : event.status === 'failed' ? 'danger' : 'warning'}
                      size="sm"
                    >
                      {event.status}
                    </Badge>
                  </TableCell>
                  <TableCell mono className="text-sm text-right">
                    {event.duration_seconds ? `${event.duration_seconds.toFixed(1)}s` : '—'}
                  </TableCell>
                  <TableCell mono className="text-sm text-right">
                    {event.keys_rotated}
                  </TableCell>
                  <TableCell mono className="text-sm text-right">
                    {event.failed_keys}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}
    </div>
  )
}
