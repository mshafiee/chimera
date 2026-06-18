import { Card, CardHeader, CardTitle, CardContent } from '../ui/Card'
import { Badge } from '../ui/Badge'
import { RealtimeBadge } from '../ui/RealtimeBadge'
import { Clock, Users, TrendingUp, AlertCircle } from 'lucide-react'
import type { ScoutStatusResponse } from '../../api'

interface ScoutStatusCardProps {
  status: ScoutStatusResponse | null | undefined
  isLoading?: boolean
}

export function ScoutStatusCard({ status, isLoading }: ScoutStatusCardProps) {
  if (isLoading) {
    return (
      <Card>
        <CardContent className="p-6">
          <div className="text-center text-text-muted">Loading Scout status...</div>
        </CardContent>
      </Card>
    )
  }

  if (!status) {
    return (
      <Card>
        <CardContent className="p-6">
          <div className="text-center text-text-muted">No Scout status available</div>
        </CardContent>
      </Card>
    )
  }

  const statusVariant = {
    running: 'success',
    completed: 'success',
    failed: 'danger',
    idle: 'warning',
  } as const

  const statusColors = {
    running: 'text-profit',
    completed: 'text-shield',
    failed: 'text-loss',
    idle: 'text-spear',
  }

  return (
    <Card>
      <CardHeader>
        <div className="flex items-center justify-between">
          <CardTitle>Scout Status</CardTitle>
          <RealtimeBadge
            isLive={status.status === 'running'}
            lastUpdate={status.last_run_at ? new Date(status.last_run_at) : null}
          />
        </div>
      </CardHeader>
      <CardContent>
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-6">
          {/* Status */}
          <div className="flex items-center gap-3">
            <div className={`p-3 rounded-lg bg-surface-light`}>
              <AlertCircle className={`w-6 h-6 ${statusColors[status.status]}`} />
            </div>
            <div>
              <div className="text-xs text-text-muted">Status</div>
              <Badge variant={statusVariant[status.status]} size="sm">
                {status.status}
              </Badge>
            </div>
          </div>

          {/* Last Run */}
          <div className="flex items-center gap-3">
            <div className="p-3 rounded-lg bg-surface-light">
              <Clock className="w-6 h-6 text-text-muted" />
            </div>
            <div>
              <div className="text-xs text-text-muted">Last Run</div>
              <div className="text-sm font-medium">
                {status.last_run_at
                  ? new Date(status.last_run_at).toLocaleString()
                  : 'Never'}
              </div>
            </div>
          </div>

          {/* Wallets Analyzed */}
          <div className="flex items-center gap-3">
            <div className="p-3 rounded-lg bg-surface-light">
              <Users className="w-6 h-6 text-text-muted" />
            </div>
            <div>
              <div className="text-xs text-text-muted">Analyzed</div>
              <div className="text-sm font-medium">{status.wallets_analyzed} wallets</div>
            </div>
          </div>

          {/* Duration */}
          <div className="flex items-center gap-3">
            <div className="p-3 rounded-lg bg-surface-light">
              <TrendingUp className="w-6 h-6 text-text-muted" />
            </div>
            <div>
              <div className="text-xs text-text-muted">Duration</div>
              <div className="text-sm font-medium">
                {status.analysis_duration_seconds.toFixed(1)}s
              </div>
            </div>
          </div>
        </div>

        {/* Next Run */}
        {status.next_run_at && (
          <div className="mt-4 pt-4 border-t border-border">
            <div className="flex items-center justify-between text-sm">
              <span className="text-text-muted">Next scheduled run:</span>
              <span className="font-medium">
                {new Date(status.next_run_at).toLocaleString()}
              </span>
            </div>
          </div>
        )}
      </CardContent>
    </Card>
  )
}
