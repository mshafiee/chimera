import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../ui/Table'
import { Badge } from '../ui/Badge'
import type { RPCLatencyResponse } from '../../api'

interface RPCLatencyTableProps {
  data: RPCLatencyResponse
}

export function RPCLatencyTable({ data }: RPCLatencyTableProps) {
  return (
    <div className="space-y-4">
      {/* Overall Stats */}
      <div className="grid grid-cols-4 gap-4">
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Overall Avg</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.overall_avg.toFixed(0)}ms
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Overall p95</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.overall_p95.toFixed(0)}ms
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Overall p99</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.overall_p99.toFixed(0)}ms
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Error Rate</div>
          <div className={`text-xl font-semibold font-mono-numbers ${
            data.error_rate < 0.01 ? 'text-profit' : data.error_rate < 0.05 ? 'text-spear' : 'text-loss'
          }`}>
            {(data.error_rate * 100).toFixed(2)}%
          </div>
        </div>
      </div>

      {/* Endpoint Breakdown */}
      <Table>
        <TableHeader>
          <TableRow hoverable={false}>
            <TableHead>Endpoint</TableHead>
            <TableHead className="text-right">Avg Latency</TableHead>
            <TableHead className="text-right">p95</TableHead>
            <TableHead className="text-right">p99</TableHead>
            <TableHead className="text-right">Error Rate</TableHead>
            <TableHead className="text-right">Requests</TableHead>
            <TableHead className="text-right">Success Rate</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {data.endpoints.map((endpoint) => (
            <TableRow key={endpoint.endpoint}>
              <TableCell className="font-medium">{endpoint.endpoint}</TableCell>
              <TableCell mono className="text-sm text-right">
                {endpoint.avg_latency_ms.toFixed(0)}ms
              </TableCell>
              <TableCell mono className="text-sm text-right">
                {endpoint.p95_latency_ms.toFixed(0)}ms
              </TableCell>
              <TableCell mono className="text-sm text-right">
                {endpoint.p99_latency_ms.toFixed(0)}ms
              </TableCell>
              <TableCell className="text-right">
                <Badge variant={endpoint.error_rate < 0.01 ? 'success' : endpoint.error_rate < 0.05 ? 'warning' : 'danger'} size="sm">
                  {(endpoint.error_rate * 100).toFixed(2)}%
                </Badge>
              </TableCell>
              <TableCell mono className="text-sm text-right">
                {endpoint.request_count}
              </TableCell>
              <TableCell className="text-right">
                <span className={endpoint.success_rate >= 0.99 ? 'text-profit' : endpoint.success_rate >= 0.95 ? 'text-spear' : 'text-loss'}>
                  {(endpoint.success_rate * 100).toFixed(2)}%
                </span>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  )
}
