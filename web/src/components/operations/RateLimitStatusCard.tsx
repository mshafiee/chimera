import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../ui/Table'
import { Badge } from '../ui/Badge'
import type { RateLimitStatusResponse } from '../../api'

interface RateLimitStatusCardProps {
  data: RateLimitStatusResponse
}

export function RateLimitStatusCard({ data }: RateLimitStatusCardProps) {
  const getEndpointStatus = (status: string) => {
    switch (status) {
      case 'ok': return 'success'
      case 'warning': return 'warning'
      case 'throttled': return 'danger'
      default: return 'default'
    }
  }

  return (
    <div className="space-y-4">
      {/* Overall Status */}
      <div className="flex items-center justify-between">
        <div className="text-sm text-text-muted">Overall Status</div>
        <Badge
          variant={data.overall_status === 'healthy' ? 'success' : data.overall_status === 'degraded' ? 'warning' : 'danger'}
          size="md"
        >
          {data.overall_status}
        </Badge>
      </div>

      {/* Rate Limits by Endpoint */}
      <Table>
        <TableHeader>
          <TableRow hoverable={false}>
            <TableHead>Endpoint</TableHead>
            <TableHead className="text-right">Current</TableHead>
            <TableHead className="text-right">Limit</TableHead>
            <TableHead className="text-right">Remaining</TableHead>
            <TableHead className="text-right">Utilization</TableHead>
            <TableHead className="text-right">Reset</TableHead>
            <TableHead>Status</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {data.endpoints.map((endpoint) => (
            <TableRow key={endpoint.endpoint}>
              <TableCell className="font-medium">{endpoint.endpoint}</TableCell>
              <TableCell mono className="text-sm text-right">
                {endpoint.current_rate.toFixed(1)}
              </TableCell>
              <TableCell mono className="text-sm text-right">
                {endpoint.limit}
              </TableCell>
              <TableCell mono className="text-sm text-right">
                {endpoint.remaining}
              </TableCell>
              <TableCell className="text-right">
                <Badge
                  variant={endpoint.utilization_percent < 50 ? 'success' : endpoint.utilization_percent < 80 ? 'warning' : 'danger'}
                  size="sm"
                >
                  {endpoint.utilization_percent.toFixed(0)}%
                </Badge>
              </TableCell>
              <TableCell className="text-sm text-text-muted">
                {new Date(endpoint.reset_at).toLocaleTimeString()}
              </TableCell>
              <TableCell>
                <Badge variant={getEndpointStatus(endpoint.status)} size="sm">
                  {endpoint.status}
                </Badge>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  )
}
