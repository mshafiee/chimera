import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../ui/Table'
import { Badge } from '../ui/Badge'
import type { HealthCheckDetailsResponse } from '../../api'
import { CheckCircle2, XCircle, AlertTriangle, Clock } from 'lucide-react'

interface HealthChecksCardProps {
  data: HealthCheckDetailsResponse
}

const STATUS_CONFIG = {
  passing: {
    icon: CheckCircle2,
    color: 'text-profit',
    bgColor: 'bg-profit/10',
    borderColor: 'border-profit/20',
    variant: 'success' as const,
    label: 'Passing'
  },
  warning: {
    icon: AlertTriangle,
    color: 'text-spear',
    bgColor: 'bg-spear/10',
    borderColor: 'border-spear/20',
    variant: 'warning' as const,
    label: 'Warning'
  },
  failing: {
    icon: XCircle,
    color: 'text-loss',
    bgColor: 'bg-loss/10',
    borderColor: 'border-loss/20',
    variant: 'danger' as const,
    label: 'Failing'
  },
}

export function HealthChecksCard({ data }: HealthChecksCardProps) {
  const getStatusConfig = (status: string) => {
    return STATUS_CONFIG[status as keyof typeof STATUS_CONFIG] || STATUS_CONFIG.failing
  }

  const overallConfig = {
    healthy: { ...STATUS_CONFIG.passing, label: 'Healthy' },
    degraded: { ...STATUS_CONFIG.warning, label: 'Degraded' },
    unhealthy: { ...STATUS_CONFIG.failing, label: 'Unhealthy' },
  }[data.overall_status] || STATUS_CONFIG.failing

  const OverallIcon = overallConfig.icon

  return (
    <div className="space-y-4">
      {/* Overall Status - Enhanced */}
      <div className={`flex items-center justify-between p-3 rounded-lg border ${overallConfig.bgColor} ${overallConfig.borderColor} hover:shadow-md transition-shadow duration-200`}>
        <div className="flex items-center gap-3">
          <div className={`p-2 rounded-full ${overallConfig.bgColor}`}>
            <OverallIcon className={`w-5 h-5 ${overallConfig.color}`} />
          </div>
          <div>
            <div className="text-xs text-text-muted">Overall System Health</div>
            <Badge variant={overallConfig.variant} size="md">
              {overallConfig.label}
            </Badge>
          </div>
        </div>
        <div className="text-right">
          <div className="text-xs text-text-muted">Checks</div>
          <div className="text-lg font-semibold font-mono-numbers">{data.checks.length}</div>
        </div>
      </div>

      {/* Health Checks Summary */}
      <div className="grid grid-cols-3 gap-3">
        {Object.entries(
          data.checks.reduce((acc, check) => {
            acc[check.status] = (acc[check.status] || 0) + 1
            return acc
          }, {} as Record<string, number>)
        ).map(([status, count]) => {
          const config = getStatusConfig(status)
          return (
            <div key={status} className={`bg-surface-light rounded-lg p-3 text-center hover:shadow-md transition-shadow duration-200`}>
              <div className="text-xs text-text-muted mb-1 capitalize">{status}</div>
              <div className={`text-xl font-semibold font-mono-numbers ${config.color}`}>
                {count}
              </div>
            </div>
          )
        })}
      </div>

      {/* Health Checks - Enhanced Table */}
      <div className="bg-surface-light rounded-lg overflow-hidden hover:shadow-md transition-shadow duration-200">
        <Table>
          <TableHeader>
            <TableRow hoverable={false}>
              <TableHead>Component</TableHead>
              <TableHead>Status</TableHead>
              <TableHead className="text-right">Response Time</TableHead>
              <TableHead>Last Check</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {data.checks.map((check) => {
              const config = getStatusConfig(check.status)
              const Icon = config.icon

              return (
                <TableRow key={check.name}>
                  <TableCell className="font-medium text-sm">{check.name}</TableCell>
                  <TableCell>
                    <div className="flex items-center gap-2">
                      <div className={`p-1.5 rounded-full ${config.bgColor}`}>
                        <Icon className={`w-3 h-3 ${config.color}`} />
                      </div>
                      <Badge variant={config.variant} size="sm">
                        {config.label}
                      </Badge>
                    </div>
                  </TableCell>
                  <TableCell className="text-right">
                    <div className="flex items-center justify-end gap-1">
                      <span className="font-mono-numbers text-sm">{check.response_time_ms.toFixed(0)}</span>
                      <span className="text-xs text-text-muted">ms</span>
                    </div>
                  </TableCell>
                  <TableCell className="text-sm text-text-muted">
                    <div className="flex items-center gap-1">
                      <Clock className="w-3 h-3" />
                      {new Date(check.last_check).toLocaleTimeString()}
                    </div>
                  </TableCell>
                </TableRow>
              )
            })}
          </TableBody>
        </Table>
      </div>

      {/* Detailed Status Messages */}
      <div className="space-y-2">
        {data.checks.filter(check => check.message).slice(0, 2).map((check) => {
          const config = getStatusConfig(check.status)
          return (
            <div key={check.name} className={`flex items-start gap-2 p-2 rounded-lg ${config.bgColor} ${config.borderColor} border`}>
              <config.icon className={`w-4 h-4 ${config.color} mt-0.5`} />
              <div className="flex-1">
                <div className="text-sm font-medium capitalize">{check.name}</div>
                <div className="text-xs text-text-muted">{check.message}</div>
              </div>
            </div>
          )
        })}
      </div>
    </div>
  )
}
