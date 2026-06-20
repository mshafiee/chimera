import { MetricCard } from '../ui/MetricCard'
import type { WebhookStats } from '../../api'
import { GitBranch, CheckCircle, AlertTriangle, XCircle } from 'lucide-react'

interface WebhookStatsCardProps {
  data: WebhookStats | null | undefined
  isLoading: boolean
}

export function WebhookStatsCard({ data, isLoading }: WebhookStatsCardProps) {
  return (
    <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
      {/* Total Webhooks */}
      <MetricCard
        label="Total Webhooks"
        value={data?.total_webhooks ?? 0}
        icon={<GitBranch className="w-4 h-4" />}
        loading={isLoading}
      />

      {/* Active Webhooks */}
      <MetricCard
        label="Active Webhooks"
        value={data?.active_webhooks ?? 0}
        icon={<CheckCircle className="w-4 h-4" />}
        trend={data && data.active_webhooks > 0 ? 'up' : 'neutral'}
        positive={data ? data.active_webhooks > 0 : undefined}
        loading={isLoading}
      />

      {/* Stale Webhooks */}
      <MetricCard
        label="Stale Webhooks"
        value={data?.stale_webhooks ?? 0}
        icon={<AlertTriangle className="w-4 h-4" />}
        trend={data && data.stale_webhooks > 0 ? 'down' : 'neutral'}
        positive={data ? data.stale_webhooks === 0 : undefined}
        loading={isLoading}
      />

      {/* Failed Registrations */}
      <MetricCard
        label="Failed Registrations"
        value={data?.failed_registrations ?? 0}
        icon={<XCircle className="w-4 h-4" />}
        trend={data && data.failed_registrations > 0 ? 'down' : 'neutral'}
        positive={data ? data.failed_registrations === 0 : undefined}
        loading={isLoading}
      />
    </div>
  )
}
