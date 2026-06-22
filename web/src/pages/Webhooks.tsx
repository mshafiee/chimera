import { useState, useEffect } from 'react'
import { Card, CardHeader, CardTitle, CardContent } from '../components/ui/Card'
import { Button } from '../components/ui/Button'
import {
  useWebhookStats,
  useWebhookAuditLog,
  useBulkRegisterWebhooks,
  useBulkCleanupWebhooks,
  useReconcileWebhooks,
  useHealthCheckWebhooks,
  type BulkRegisterRequest,
  type BulkCleanupRequest,
} from '../api'
import {
  WebhookStatsCard,
  WebhookAuditTable,
  BulkOperationModal,
  WebhookHealthCard,
} from '../components/webhooks'
import {
  GitBranch,
  RefreshCw,
  Database,
  Plus,
  Trash2,
  Loader2,
} from 'lucide-react'
import { toast } from '../components/ui/Toast'
import { useLayoutContext } from '../components/layout/Layout'
import type { WebhookAction, WebhookStatus, HealthCheckResult, BulkOperationResult } from '../api'

export function Webhooks() {
  const { setLastUpdate } = useLayoutContext()
  const [bulkModalOpen, setBulkModalOpen] = useState(false)
  const [bulkOperation, setBulkOperation] = useState<'register' | 'cleanup'>('register')
  const [isReconciling, setIsReconciling] = useState(false)
  const [isHealthChecking, setIsHealthChecking] = useState(false)

  // Filters for audit log
  const [actionFilter, setActionFilter] = useState<WebhookAction | 'all'>('all')
  const [statusFilter, setStatusFilter] = useState<WebhookStatus | 'all'>('all')

  // Fetch webhook statistics
  const { data: stats, isLoading: statsLoading, refetch: refetchStats } = useWebhookStats(30000)

  // Fetch audit log
  const { data: auditLog, isLoading: auditLoading, refetch: refetchAudit } = useWebhookAuditLog({
    action: actionFilter === 'all' ? undefined : actionFilter,
    status: statusFilter === 'all' ? undefined : statusFilter,
    limit: 50,
  })

  // Update last update time
  useEffect(() => {
    if (stats || auditLog) {
      setLastUpdate(new Date())
    }
  }, [stats, auditLog, setLastUpdate])

  // Mutations
  const bulkRegister = useBulkRegisterWebhooks()
  const bulkCleanup = useBulkCleanupWebhooks()
  const reconcile = useReconcileWebhooks()
  const healthCheck = useHealthCheckWebhooks()

  // Handlers
  const handleBulkOperation = async (request: BulkRegisterRequest | BulkCleanupRequest): Promise<BulkOperationResult> => {
    if (bulkOperation === 'register') {
      const result = await bulkRegister.mutateAsync(request)
      toast.success(`Webhook registration complete`)
      setBulkModalOpen(false)
      refetchStats()
      refetchAudit()
      return result
    } else {
      const result = await bulkCleanup.mutateAsync(request)
      toast.success(`Webhook cleanup complete`)
      setBulkModalOpen(false)
      refetchStats()
      refetchAudit()
      return result
    }
  }

  const handleReconcile = async () => {
    setIsReconciling(true)
    try {
      const result = await reconcile.mutateAsync()
      toast.success(
        `Reconciliation complete: ${result.registered} registered, ${result.orphaned} orphaned, ${result.updated} updated`
      )
      refetchStats()
      refetchAudit()
    } catch (error: any) {
      toast.error(error.message || 'Reconciliation failed')
    } finally {
      setIsReconciling(false)
    }
  }

  const handleHealthCheck = async (): Promise<HealthCheckResult> => {
    setIsHealthChecking(true)
    try {
      const result = await healthCheck.mutateAsync()
      toast.success(
        `Health check complete: ${result.healthy} healthy, ${result.unhealthy} unhealthy, ${result.cleaned_up} cleaned up`
      )
      refetchStats()
      refetchAudit()
      return result
    } catch (error: any) {
      toast.error(error.message || 'Health check failed')
      throw error
    } finally {
      setIsHealthChecking(false)
    }
  }

  const openBulkModal = (operation: 'register' | 'cleanup') => {
    setBulkOperation(operation)
    setBulkModalOpen(true)
  }

  // Filter options
  const actionOptions: Array<{ value: WebhookAction | 'all'; label: string }> = [
    { value: 'all', label: 'All Actions' },
    { value: 'register', label: 'Register' },
    { value: 'update', label: 'Update' },
    { value: 'delete', label: 'Delete' },
    { value: 'toggle', label: 'Toggle' },
    { value: 'health_check', label: 'Health Check' },
    { value: 'reconcile', label: 'Reconcile' },
  ]

  const statusOptions: Array<{ value: WebhookStatus | 'all'; label: string }> = [
    { value: 'all', label: 'All Statuses' },
    { value: 'success', label: 'Success' },
    { value: 'failed', label: 'Failed' },
    { value: 'pending', label: 'Pending' },
    { value: 'retry', label: 'Retry' },
  ]

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold">Webhooks</h1>
          <p className="text-text-muted text-sm">Helius webhook lifecycle management</p>
        </div>
        <div className="flex items-center gap-3">
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              refetchStats()
              refetchAudit()
            }}
            disabled={statsLoading}
          >
            <RefreshCw className={`w-4 h-4 ${statsLoading ? 'animate-spin' : ''}`} />
          </Button>
          <Button
            variant="secondary"
            size="sm"
            onClick={handleReconcile}
            disabled={isReconciling}
          >
            {isReconciling ? (
              <>
                <Loader2 className="w-4 h-4 mr-2 animate-spin" />
                Reconciling...
              </>
            ) : (
              <>
                <Database className="w-4 h-4 mr-2" />
                Reconcile
              </>
            )}
          </Button>
        </div>
      </div>

      {/* Webhook Statistics */}
      <WebhookStatsCard data={stats} isLoading={statsLoading} />

      {/* Bulk Operations */}
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        <WebhookHealthCard
          onHealthCheck={handleHealthCheck}
          isLoading={isHealthChecking}
        />

        <Card className="hover:shadow-lg transition-shadow duration-200">
          <CardHeader>
            <div className="flex items-center gap-3">
              <div className="p-2 bg-surface-light rounded-lg">
                <GitBranch className="w-5 h-5 text-shield" />
              </div>
              <div>
                <CardTitle>Bulk Operations</CardTitle>
                <p className="text-xs text-text-muted mt-0.5">Register or cleanup multiple webhooks</p>
              </div>
            </div>
          </CardHeader>
          <CardContent>
            <div className="space-y-3">
              <div className="text-sm text-text-muted">
                Perform bulk operations on webhooks for multiple wallets at once.
              </div>
              <div className="flex gap-3">
                <Button
                  variant="primary"
                  className="flex-1"
                  onClick={() => openBulkModal('register')}
                >
                  <Plus className="w-4 h-4 mr-2" />
                  Bulk Register
                </Button>
                <Button
                  variant="secondary"
                  className="flex-1"
                  onClick={() => openBulkModal('cleanup')}
                >
                  <Trash2 className="w-4 h-4 mr-2" />
                  Bulk Cleanup
                </Button>
              </div>
            </div>
          </CardContent>
        </Card>
      </div>

      {/* Audit Log */}
      <Card>
        <CardHeader>
          <div className="flex items-center justify-between w-full">
            <CardTitle>Audit Log</CardTitle>
            <div className="flex items-center gap-2">
              {/* Action Filter */}
              <select
                value={actionFilter}
                onChange={(e) => setActionFilter(e.target.value as WebhookAction | 'all')}
                className="px-3 py-1.5 text-sm bg-surface-light border border-border rounded-lg focus:outline-none focus:ring-2 focus:ring-shield/50"
              >
                {actionOptions.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>

              {/* Status Filter */}
              <select
                value={statusFilter}
                onChange={(e) => setStatusFilter(e.target.value as WebhookStatus | 'all')}
                className="px-3 py-1.5 text-sm bg-surface-light border border-border rounded-lg focus:outline-none focus:ring-2 focus:ring-shield/50"
              >
                {statusOptions.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
            </div>
          </div>
        </CardHeader>
        <CardContent>
          <WebhookAuditTable logs={auditLog} isLoading={auditLoading} />
        </CardContent>
      </Card>

      {/* Bulk Operation Modal */}
      <BulkOperationModal
        isOpen={bulkModalOpen}
        onClose={() => setBulkModalOpen(false)}
        operation={bulkOperation}
        onConfirm={handleBulkOperation}
      />
    </div>
  )
}
