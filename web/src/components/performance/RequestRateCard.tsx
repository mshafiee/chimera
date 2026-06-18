import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../ui/Table'
import { Badge } from '../ui/Badge'
import type { RequestRateResponse } from '../../api'

interface RequestRateCardProps {
  data: RequestRateResponse
}

export function RequestRateCard({ data }: RequestRateCardProps) {
  return (
    <div className="space-y-4">
      {/* Overall Stats */}
      <div className="grid grid-cols-4 gap-4">
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Current RPS</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.current_rps.toFixed(1)}
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Peak RPS</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.peak_rps.toFixed(1)}
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Avg RPS</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.avg_rps.toFixed(1)}
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Status</div>
          <Badge
            variant={data.overall_status === 'healthy' ? 'success' : data.overall_status === 'degraded' ? 'warning' : 'danger'}
            size="lg"
          >
            {data.overall_status}
          </Badge>
        </div>
      </div>

      {/* Rate Limits by Endpoint */}
      {data.rate_limits.length > 0 && (
        <Table>
          <TableHeader>
            <TableRow hoverable={false}>
              <TableHead>Endpoint</TableHead>
              <TableHead className="text-right">Current Rate</TableHead>
              <TableHead className="text-right">Limit</TableHead>
              <TableHead className="text-right">Utilization</TableHead>
              <TableHead>Status</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {data.rate_limits.map((limit) => (
              <TableRow key={limit.endpoint}>
                <TableCell className="font-medium">{limit.endpoint}</TableCell>
                <TableCell mono className="text-sm text-right">
                  {limit.current_rate.toFixed(1)}
                </TableCell>
                <TableCell mono className="text-sm text-right">
                  {limit.limit}
                </TableCell>
                <TableCell className="text-right">
                  <Badge
                    variant={limit.utilization_percent < 70 ? 'success' : limit.utilization_percent < 90 ? 'warning' : 'danger'}
                    size="sm"
                  >
                    {limit.utilization_percent.toFixed(0)}%
                  </Badge>
                </TableCell>
                <TableCell>
                  <Badge
                    variant={limit.status === 'ok' ? 'success' : limit.status === 'warning' ? 'warning' : 'danger'}
                    size="sm"
                  >
                    {limit.status}
                  </Badge>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
    </div>
  )
}
