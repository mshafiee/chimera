import { Badge } from '../ui/Badge'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../ui/Table'
import type { SecretRotationResponse } from '../../api'
import { Clock, CheckCircle, XCircle, Calendar, History } from 'lucide-react'

interface SecretRotationCardProps {
  data: SecretRotationResponse
}

export function SecretRotationCard({ data }: SecretRotationCardProps) {
  const getStatusConfig = (status: string) => {
    switch (status) {
      case 'active':
        return { icon: CheckCircle, color: 'text-profit', label: 'Active', bgColor: 'bg-profit/10', borderColor: 'border-profit/20' }
      case 'due_soon':
        return { icon: Clock, color: 'text-spear', label: 'Due Soon', bgColor: 'bg-spear/10', borderColor: 'border-spear/20' }
      case 'overdue':
        return { icon: XCircle, color: 'text-loss', label: 'Overdue', bgColor: 'bg-loss/10', borderColor: 'border-loss/20' }
      default:
        return { icon: Clock, color: 'text-text-muted', label: 'Unknown', bgColor: 'bg-surface', borderColor: 'border-border' }
    }
  }

  const statusConfig = getStatusConfig(data.status)
  const StatusIcon = statusConfig.icon

  return (
    <div className="space-y-6">
      {/* Current Status - Enhanced */}
      <div className={`flex items-center gap-4 p-4 rounded-lg border ${statusConfig.bgColor} ${statusConfig.borderColor} hover:shadow-md transition-shadow duration-200`}>
        <div className={`p-3 rounded-full ${statusConfig.bgColor}`}>
          <StatusIcon className={`w-6 h-6 ${statusConfig.color}`} />
        </div>
        <div className="flex-1">
          <div className="text-sm text-text-muted mb-1">Current Rotation Status</div>
          <div className="flex items-center gap-2">
            <Badge variant={data.status === 'active' ? 'success' : data.status === 'due_soon' ? 'warning' : data.status === 'overdue' ? 'danger' : 'default'} size="md">
              {statusConfig.label}
            </Badge>
            {data.days_until_due !== null && (
              <span className="text-sm text-text-muted">
                {data.days_until_due < 0
                  ? `${Math.abs(data.days_until_due)} days overdue`
                  : `${data.days_until_due} days remaining`
                }
              </span>
            )}
          </div>
        </div>
        {data.days_until_due !== null && (
          <div className="text-center">
            <div className="text-2xl font-bold font-mono-numbers">
              {data.days_until_due < 0 ? Math.abs(data.days_until_due) : data.days_until_due}
            </div>
            <div className="text-xs text-text-muted">Days</div>
          </div>
        )}
      </div>

      {/* Timeline */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        {/* Last Rotation */}
        <div className="bg-surface-light rounded-lg p-4 hover:shadow-md transition-shadow duration-200">
          <div className="flex items-center gap-2 mb-2">
            <Calendar className="w-4 h-4 text-text-muted" />
            <span className="text-sm font-medium">Last Rotation</span>
          </div>
          <div className="text-base font-semibold">
            {data.last_rotation_at
              ? new Date(data.last_rotation_at).toLocaleDateString()
              : 'Never'}
          </div>
          {data.last_rotation_at && (
            <div className="text-xs text-text-muted mt-1">
              {new Date(data.last_rotation_at).toLocaleTimeString()}
            </div>
          )}
        </div>

        {/* Next Rotation */}
        <div className="bg-surface-light rounded-lg p-4 hover:shadow-md transition-shadow duration-200">
          <div className="flex items-center gap-2 mb-2">
            <Calendar className="w-4 h-4 text-text-muted" />
            <span className="text-sm font-medium">Next Rotation</span>
          </div>
          <div className="text-base font-semibold">
            {data.next_rotation_at
              ? new Date(data.next_rotation_at).toLocaleDateString()
              : 'Not scheduled'}
          </div>
          {data.next_rotation_at && (
            <div className="text-xs text-text-muted mt-1">
              {new Date(data.next_rotation_at).toLocaleTimeString()}
            </div>
          )}
        </div>
      </div>

      {/* Rotation History */}
      {data.rotation_history.length > 0 && (
        <div>
          <div className="flex items-center gap-2 mb-3">
            <History className="w-4 h-4 text-text-muted" />
            <h3 className="text-sm font-medium">Rotation History</h3>
            <span className="text-xs text-text-muted">({data.rotation_history.length} events)</span>
          </div>
          <div className="bg-surface-light rounded-lg overflow-hidden hover:shadow-md transition-shadow duration-200">
            <Table>
              <TableHeader>
                <TableRow hoverable={false}>
                  <TableHead>Timestamp</TableHead>
                  <TableHead>Status</TableHead>
                  <TableHead className="text-right">Duration</TableHead>
                  <TableHead className="text-right">Keys</TableHead>
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
        </div>
      )}
    </div>
  )
}
