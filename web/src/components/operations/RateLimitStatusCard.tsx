import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../ui/Table'
import { Badge } from '../ui/Badge'
import type { RateLimitStatusResponse } from '../../api'
import { TrendingUp, AlertTriangle, XCircle } from 'lucide-react'

interface RateLimitStatusCardProps {
  data: RateLimitStatusResponse
}

const getEndpointStatus = (status: string) => {
  switch (status) {
    case 'ok': return 'success'
    case 'warning': return 'warning'
    case 'throttled': return 'danger'
    default: return 'default'
  }
}

const getUtilizationVariant = (percent: number) => {
  if (percent < 50) return 'success'
  if (percent < 80) return 'warning'
  return 'danger'
}

const getOverallIcon = (status: string) => {
  switch (status) {
    case 'healthy': return TrendingUp
    case 'degraded': return AlertTriangle
    case 'throttled': return XCircle
    default: return TrendingUp
  }
}

export function RateLimitStatusCard({ data }: RateLimitStatusCardProps) {
  const OverallIcon = getOverallIcon(data.overall_status)

  return (
    <div className="space-y-4">
      {/* Overall Status - Enhanced */}
      <div className={`flex items-center justify-between p-3 rounded-lg border ${
        data.overall_status === 'healthy' ? 'bg-profit/10 border-profit/20' :
        data.overall_status === 'degraded' ? 'bg-spear/10 border-spear/20' :
        'bg-loss/10 border-loss/20'
      }`}>
        <div className="flex items-center gap-3">
          <div className={`p-2 rounded-full ${
            data.overall_status === 'healthy' ? 'bg-profit/20' :
            data.overall_status === 'degraded' ? 'bg-spear/20' :
            'bg-loss/20'
          }`}>
            <OverallIcon className={`w-4 h-4 ${
              data.overall_status === 'healthy' ? 'text-profit' :
              data.overall_status === 'degraded' ? 'text-spear' :
              'text-loss'
            }`} />
          </div>
          <div>
            <div className="text-xs text-text-muted">Overall Status</div>
            <Badge
              variant={data.overall_status === 'healthy' ? 'success' : data.overall_status === 'degraded' ? 'warning' : 'danger'}
              size="sm"
            >
              {data.overall_status}
            </Badge>
          </div>
        </div>
        <div className="text-right">
          <div className="text-xs text-text-muted">Endpoints</div>
          <div className="text-lg font-semibold font-mono-numbers">{data.endpoints.length}</div>
        </div>
      </div>

      {/* Rate Limits by Endpoint - Enhanced Table */}
      <div className="bg-surface-light rounded-lg overflow-hidden hover:shadow-md transition-shadow duration-200">
        <Table>
          <TableHeader>
            <TableRow hoverable={false}>
              <TableHead>Endpoint</TableHead>
              <TableHead className="text-right">Rate</TableHead>
              <TableHead className="text-right">Limit</TableHead>
              <TableHead className="text-right">Utilization</TableHead>
              <TableHead>Status</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {data.endpoints.map((endpoint) => (
              <TableRow key={endpoint.endpoint}>
                <TableCell className="font-medium text-sm">{endpoint.endpoint}</TableCell>
                <TableCell className="text-right">
                  <div className="flex flex-col">
                    <span className="font-mono-numbers text-sm font-medium">{endpoint.current_rate.toFixed(1)}</span>
                    <span className="text-xs text-text-muted">req/s</span>
                  </div>
                </TableCell>
                <TableCell className="text-right">
                  <div className="flex flex-col">
                    <span className="font-mono-numbers text-sm text-text-muted">{endpoint.limit}</span>
                    <span className="text-xs text-text-muted">req/s</span>
                  </div>
                </TableCell>
                <TableCell className="text-right">
                  <div className="flex flex-col items-end gap-1">
                    <Badge variant={getUtilizationVariant(endpoint.utilization_percent)} size="sm">
                      {endpoint.utilization_percent.toFixed(0)}%
                    </Badge>
                    <span className="text-xs text-text-muted">
                      {endpoint.remaining} remaining
                    </span>
                  </div>
                </TableCell>
                <TableCell>
                  <div className="flex items-center gap-2">
                    <Badge variant={getEndpointStatus(endpoint.status)} size="sm">
                      {endpoint.status}
                    </Badge>
                  </div>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>

      {/* Quick Stats */}
      <div className="grid grid-cols-3 gap-3">
        <div className="bg-surface-light rounded-lg p-3 text-center hover:shadow-md transition-shadow duration-200">
          <div className="text-xs text-text-muted mb-1">Total Requests</div>
          <div className="text-lg font-semibold font-mono-numbers">
            {data.endpoints.reduce((sum, ep) => sum + ep.current_rate, 0).toFixed(1)}
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-3 text-center hover:shadow-md transition-shadow duration-200">
          <div className="text-xs text-text-muted mb-1">Avg Utilization</div>
          <div className="text-lg font-semibold font-mono-numbers">
            {(data.endpoints.reduce((sum, ep) => sum + ep.utilization_percent, 0) / data.endpoints.length).toFixed(0)}%
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-3 text-center hover:shadow-md transition-shadow duration-200">
          <div className="text-xs text-text-muted mb-1">Throttled</div>
          <div className="text-lg font-semibold font-mono-numbers text-loss">
            {data.endpoints.filter(ep => ep.status === 'throttled').length}
          </div>
        </div>
      </div>
    </div>
  )
}
