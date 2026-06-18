import { Card, CardHeader, CardTitle, CardContent } from '../components/ui/Card'
import { Badge } from '../components/ui/Badge'
import { useResourceUsage, useSecretRotation, useRateLimitStatus, useHealthCheckDetails } from '../api'
import { ResourceUsageCard } from '../components/operations/ResourceUsageCard'
import { SecretRotationCard } from '../components/operations/SecretRotationCard'
import { RateLimitStatusCard } from '../components/operations/RateLimitStatusCard'
import { HealthChecksCard } from '../components/operations/HealthChecksCard'
import { MetricCard } from '../components/ui/MetricCard'
import { Activity, Key, Zap, Heartbeat } from 'lucide-react'

export function Operations() {
  const { data: resourceUsage, isLoading: resourcesLoading } = useResourceUsage(10000)
  const { data: secretRotation, isLoading: rotationLoading } = useSecretRotation()
  const { data: rateLimitStatus, isLoading: rateLoading } = useRateLimitStatus()
  const { data: healthChecks, isLoading: healthLoading } = useHealthCheckDetails()

  return (
    <div className="space-y-6">
      {/* Header */}
      <div>
        <h1 className="text-2xl font-bold">Operations</h1>
        <p className="text-text-muted text-sm">System resources and operational health</p>
      </div>

      {/* Resource Usage */}
      <Card>
        <CardHeader>
          <div className="flex items-center gap-2">
            <Activity className="w-5 h-5 text-text-muted" />
            <CardTitle>Resource Usage</CardTitle>
          </div>
        </CardHeader>
        <CardContent>
          {resourcesLoading ? (
            <div className="text-center text-text-muted py-8">Loading resource data...</div>
          ) : resourceUsage ? (
            <ResourceUsageCard data={resourceUsage} />
          ) : (
            <div className="text-center text-text-muted py-8">No resource data available</div>
          )}
        </CardContent>
      </Card>

      {/* Secret Rotation */}
      <Card>
        <CardHeader>
          <div className="flex items-center gap-2">
            <Key className="w-5 h-5 text-text-muted" />
            <CardTitle>Secret Rotation</CardTitle>
          </div>
        </CardHeader>
        <CardContent>
          {rotationLoading ? (
            <div className="text-center text-text-muted py-8">Loading rotation data...</div>
          ) : secretRotation ? (
            <SecretRotationCard data={secretRotation} />
          ) : (
            <div className="text-center text-text-muted py-8">No rotation data available</div>
          )}
        </CardContent>
      </Card>

      {/* Rate Limit Status */}
      <Card>
        <CardHeader>
          <div className="flex items-center gap-2">
            <Zap className="w-5 h-5 text-text-muted" />
            <CardTitle>Rate Limit Status</CardTitle>
          </div>
        </CardHeader>
        <CardContent>
          {rateLoading ? (
            <div className="text-center text-text-muted py-8">Loading rate limit data...</div>
          ) : rateLimitStatus ? (
            <RateLimitStatusCard data={rateLimitStatus} />
          ) : (
            <div className="text-center text-text-muted py-8">No rate limit data available</div>
          )}
        </CardContent>
      </Card>

      {/* Health Checks */}
      <Card>
        <CardHeader>
          <div className="flex items-center gap-2">
            <Heartbeat className="w-5 h-5 text-text-muted" />
            <CardTitle>Health Checks</CardTitle>
          </div>
        </CardHeader>
        <CardContent>
          {healthLoading ? (
            <div className="text-center text-text-muted py-8">Loading health checks...</div>
          ) : healthChecks ? (
            <HealthChecksCard data={healthChecks} />
          ) : (
            <div className="text-center text-text-muted py-8">No health check data available</div>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
