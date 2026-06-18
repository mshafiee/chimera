import { Badge } from '../ui/Badge'
import type { RPCLatencyResponse } from '../../api'

interface RPCLatencyMiniProps {
  data: RPCLatencyResponse
}

export function RPCLatencyMini({ data }: RPCLatencyMiniProps) {
  return (
    <div className="flex items-center gap-4">
      <div className="text-sm text-text-muted">RPC Latency</div>
      <Badge
        variant={data.overall_avg < 50 ? 'success' : data.overall_avg < 100 ? 'warning' : 'danger'}
        size="sm"
      >
        {data.overall_avg.toFixed(0)}ms
      </Badge>
    </div>
  )
}
