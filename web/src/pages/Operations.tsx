import { Card, CardHeader, CardTitle, CardContent } from '../components/ui/Card'
import { useResourceUsage, useSecretRotation, useRateLimitStatus, useHealthCheckDetails } from '../api'
import { ResourceUsageCard } from '../components/operations/ResourceUsageCard'
import { SecretRotationCard } from '../components/operations/SecretRotationCard'
import { RateLimitStatusCard } from '../components/operations/RateLimitStatusCard'
import { HealthChecksCard } from '../components/operations/HealthChecksCard'
import { Activity, Key, Zap, Heart, AlertCircle, CheckCircle2 } from 'lucide-react'
import { Badge } from '../components/ui/Badge'

export function Operations() {
  const { data: resourceUsage, isLoading: resourcesLoading } = useResourceUsage(10000)
  const { data: secretRotation, isLoading: rotationLoading } = useSecretRotation()
  const { data: rateLimitStatus, isLoading: rateLoading } = useRateLimitStatus()
  const { data: healthChecks, isLoading: healthLoading } = useHealthCheckDetails()

  // Calculate overall system status
  const getSystemStatus = () => {
    if (healthLoading || !healthChecks) return { status: 'loading', label: 'Loading', variant: 'default' as const }

    const failingChecks = healthChecks.checks.filter(c => c.status === 'failing').length
    const warningChecks = healthChecks.checks.filter(c => c.status === 'warning').length

    if (failingChecks > 0) {
      return { status: 'critical', label: 'Critical', variant: 'danger' as const }
    } else if (warningChecks > 0) {
      return { status: 'degraded', label: 'Degraded', variant: 'warning' as const }
    } else {
      return { status: 'healthy', label: 'Healthy', variant: 'success' as const }
    }
  }

  const systemStatus = getSystemStatus()

  return (
    <div className="space-y-6">
      {/* Header with System Status */}
      <div className="flex items-start justify-between">
        <div>
          <h1 className="text-3xl font-bold tracking-tight">Operations</h1>
          <p className="text-text-muted text-sm mt-1">System resources and operational health</p>
        </div>
        <div className="flex items-center gap-3 bg-surface-light rounded-lg px-4 py-2">
          {systemStatus.status === 'healthy' ? (
            <CheckCircle2 className="w-5 h-5 text-profit" />
          ) : systemStatus.status === 'critical' ? (
            <AlertCircle className="w-5 h-5 text-loss" />
          ) : (
            <Activity className="w-5 h-5 text-spear" />
          )}
          <div>
            <div className="text-xs text-text-muted">System Status</div>
            <Badge variant={systemStatus.variant} size="sm">{systemStatus.label}</Badge>
          </div>
        </div>
      </div>

      {/* Main Grid Layout */}
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        {/* Resource Usage - Full Width on Mobile, Half on Desktop */}
        <Card className="lg:col-span-2 hover:shadow-lg transition-shadow duration-200">
          <CardHeader>
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-3">
                <div className="p-2 bg-surface-light rounded-lg">
                  <Activity className="w-5 h-5 text-profit" />
                </div>
                <div>
                  <CardTitle>Resource Usage</CardTitle>
                  <p className="text-xs text-text-muted mt-0.5">Real-time system metrics</p>
                </div>
              </div>
              {!resourcesLoading && resourceUsage && (
                <div className="text-right ml-4">
                  <div className="text-xs text-text-muted">Last updated</div>
                  <div className="text-sm font-mono-numbers">
                    {new Date(resourceUsage.timestamp).toLocaleTimeString()}
                  </div>
                </div>
              )}
            </div>
          </CardHeader>
          <CardContent>
            {resourcesLoading ? (
              <div className="flex items-center justify-center py-12">
                <div className="flex items-center gap-3 text-text-muted">
                  <div className="animate-spin w-5 h-5 border-2 border-current border-t-transparent rounded-full" />
                  <span>Loading resource data...</span>
                </div>
              </div>
            ) : resourceUsage ? (
              <ResourceUsageCard data={resourceUsage} />
            ) : (
              <div className="text-center text-text-muted py-8">
                <AlertCircle className="w-8 h-8 mx-auto mb-2 opacity-50" />
                No resource data available
              </div>
            )}
          </CardContent>
        </Card>

        {/* Health Checks - Prioritized */}
        <Card className="hover:shadow-lg transition-shadow duration-200">
          <CardHeader>
            <div className="flex items-center gap-3">
              <div className="p-2 bg-surface-light rounded-lg">
                <Heart className="w-5 h-5 text-loss" />
              </div>
              <div>
                <CardTitle>Health Checks</CardTitle>
                <p className="text-xs text-text-muted mt-0.5">Component status</p>
              </div>
            </div>
          </CardHeader>
          <CardContent>
            {healthLoading ? (
              <div className="flex items-center justify-center py-12">
                <div className="flex items-center gap-3 text-text-muted">
                  <div className="animate-spin w-5 h-5 border-2 border-current border-t-transparent rounded-full" />
                  <span>Loading health checks...</span>
                </div>
              </div>
            ) : healthChecks ? (
              <HealthChecksCard data={healthChecks} />
            ) : (
              <div className="text-center text-text-muted py-8">
                <AlertCircle className="w-8 h-8 mx-auto mb-2 opacity-50" />
                No health check data available
              </div>
            )}
          </CardContent>
        </Card>

        {/* Rate Limit Status */}
        <Card className="hover:shadow-lg transition-shadow duration-200">
          <CardHeader>
            <div className="flex items-center gap-3">
              <div className="p-2 bg-surface-light rounded-lg">
                <Zap className="w-5 h-5 text-spear" />
              </div>
              <div>
                <CardTitle>Rate Limits</CardTitle>
                <p className="text-xs text-text-muted mt-0.5">API endpoint throttling</p>
              </div>
            </div>
          </CardHeader>
          <CardContent>
            {rateLoading ? (
              <div className="flex items-center justify-center py-12">
                <div className="flex items-center gap-3 text-text-muted">
                  <div className="animate-spin w-5 h-5 border-2 border-current border-t-transparent rounded-full" />
                  <span>Loading rate limit data...</span>
                </div>
              </div>
            ) : rateLimitStatus ? (
              <RateLimitStatusCard data={rateLimitStatus} />
            ) : (
              <div className="text-center text-text-muted py-8">
                <AlertCircle className="w-8 h-8 mx-auto mb-2 opacity-50" />
                No rate limit data available
              </div>
            )}
          </CardContent>
        </Card>

        {/* Secret Rotation - Full Width */}
        <Card className="lg:col-span-2 hover:shadow-lg transition-shadow duration-200">
          <CardHeader>
            <div className="flex items-center gap-3">
              <div className="p-2 bg-surface-light rounded-lg">
                <Key className="w-5 h-5 text-shield" />
              </div>
              <div>
                <CardTitle>Secret Rotation</CardTitle>
                <p className="text-xs text-text-muted mt-0.5">Credential security status</p>
              </div>
            </div>
          </CardHeader>
          <CardContent>
            {rotationLoading ? (
              <div className="flex items-center justify-center py-12">
                <div className="flex items-center gap-3 text-text-muted">
                  <div className="animate-spin w-5 h-5 border-2 border-current border-t-transparent rounded-full" />
                  <span>Loading rotation data...</span>
                </div>
              </div>
            ) : secretRotation ? (
              <SecretRotationCard data={secretRotation} />
            ) : (
              <div className="text-center text-text-muted py-8">
                <AlertCircle className="w-8 h-8 mx-auto mb-2 opacity-50" />
                No rotation data available
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
