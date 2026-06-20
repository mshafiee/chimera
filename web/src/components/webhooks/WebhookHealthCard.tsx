import { Card, CardHeader, CardTitle, CardContent } from '../ui/Card'
import { Button } from '../ui/Button'
import { HeartPulse, Activity, Loader2 } from 'lucide-react'
import type { HealthCheckResult } from '../../api'

interface WebhookHealthCardProps {
  onHealthCheck: () => Promise<HealthCheckResult>
  isLoading?: boolean
}

export function WebhookHealthCard({ onHealthCheck, isLoading = false }: WebhookHealthCardProps) {
  const handleHealthCheck = async () => {
    try {
      await onHealthCheck()
    } catch (error) {
      console.error('Health check failed:', error)
    }
  }

  return (
    <Card className="hover:shadow-lg transition-shadow duration-200">
      <CardHeader>
        <div className="flex items-center justify-between w-full">
          <div className="flex items-center gap-3">
            <div className="p-2 bg-surface-light rounded-lg">
              <HeartPulse className="w-5 h-5 text-profit" />
            </div>
            <div>
              <CardTitle>Webhook Health</CardTitle>
              <p className="text-xs text-text-muted mt-0.5">Monitor webhook health status</p>
            </div>
          </div>
        </div>
      </CardHeader>
      <CardContent>
        <div className="space-y-4">
          <div className="text-sm text-text-muted">
            Run health checks on all active webhooks to identify and clean up unhealthy endpoints.
            This will check for stale webhooks and update their health status.
          </div>

          <Button
            variant="primary"
            onClick={handleHealthCheck}
            disabled={isLoading}
            className="w-full"
          >
            {isLoading ? (
              <>
                <Loader2 className="w-4 h-4 mr-2 animate-spin" />
                Running Health Check...
              </>
            ) : (
              <>
                <Activity className="w-4 h-4 mr-2" />
                Run Health Check
              </>
            )}
          </Button>
        </div>
      </CardContent>
    </Card>
  )
}
