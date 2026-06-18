import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../ui/Table'
import { Badge } from '../ui/Badge'
import type { HealthCheckDetailsResponse } from '../../api'
import { CheckCircle, XCircle, AlertTriangle } from 'lucide-react'

interface HealthChecksCardProps {
  data: HealthCheckDetailsResponse
}

const STATUS_ICONS = {
  passing: { icon: CheckCircle, color: 'text-profit', variant: 'success' as const },
  warning: { icon: AlertTriangle, color: 'text-spear', variant: 'warning' as const },
  failing: { icon: XCircle, color: 'text-loss', variant: 'danger' as const },
}

export function HealthChecksCard({ data }: HealthChecksCardProps) {
  const getStatusConfig = (status: string) => {
    return STATUS_ICONS[status as keyof typeof STATUS_ICONS] || STATUS_ICONS.failing
  }

  const overallVariant: 'default' | 'danger' | 'shield' | 'spear' | 'success' | 'warning' | 'info' =
    ({
      healthy: 'success',
      degraded: 'warning',
      unhealthy: 'danger',
    } as Record<string, 'default' | 'danger' | 'shield' | 'spear' | 'success' | 'warning' | 'info'>)[data.overall_status] || 'default'

  return (
    <div className="space-y-4">
      {/* Overall Status */}
      <div className="flex items-center justify-between">
        <div className="text-sm text-text-muted">Overall Health</div>
        <Badge variant={overallVariant} size="md">
          {data.overall_status}
        </Badge>
      </div>

      {/* Health Checks */}
      <Table>
        <TableHeader>
          <TableRow hoverable={false}>
            <TableHead>Check</TableHead>
            <TableHead>Status</TableHead>
            <TableHead>Response Time</TableHead>
            <TableHead>Last Check</TableHead>
            <TableHead>Message</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {data.checks.map((check) => {
            const config = getStatusConfig(check.status)
            const Icon = config.icon

            return (
              <TableRow key={check.name}>
                <TableCell className="font-medium">{check.name}</TableCell>
                <TableCell>
                  <div className="flex items-center gap-2">
                    <Icon className={`w-4 h-4 ${config.color}`} />
                    <Badge variant={config.variant} size="sm">
                      {check.status}
                    </Badge>
                  </div>
                </TableCell>
                <TableCell mono className="text-sm">
                  {check.response_time_ms.toFixed(0)}ms
                </TableCell>
                <TableCell className="text-sm text-text-muted">
                  {new Date(check.last_check).toLocaleString()}
                </TableCell>
                <TableCell className="text-sm text-text-muted">
                  {check.message || '—'}
                </TableCell>
              </TableRow>
            )
          })}
        </TableBody>
      </Table>
    </div>
  )
}
