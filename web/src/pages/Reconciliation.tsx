import { useState } from 'react'
import { Card, CardHeader, CardTitle, CardContent } from '../components/ui/Card'
import { Badge } from '../components/ui/Badge'
import { MetricCard } from '../components/ui/MetricCard'
import { useReconciliationStatus, useReconciliationHistory, useTriggerReconciliation } from '../api'
import { ReconciliationStatusCard } from '../components/reconciliation/ReconciliationStatusCard'
import { DiscrepanciesList } from '../components/reconciliation/DiscrepanciesList'
import { ReconciliationHistory } from '../components/reconciliation/ReconciliationHistory'
import { Play } from 'lucide-react'
import { toast } from '../components/ui/Toast'

export function Reconciliation() {
  const [historyLimit, setHistoryLimit] = useState(10)

  const { data: reconStatus, isLoading: statusLoading, refetch: refetchStatus } = useReconciliationStatus(15000)
  const { data: reconHistory, isLoading: historyLoading } = useReconciliationHistory(historyLimit)
  const triggerReconciliation = useTriggerReconciliation()

  const handleTriggerReconciliation = async () => {
    try {
      toast.info('Triggering reconciliation...')
      await triggerReconciliation.mutateAsync()
      toast.success('Reconciliation triggered successfully')
      refetchStatus()
    } catch (error) {
      toast.error('Failed to trigger reconciliation')
    }
  }

  const discrepancies = reconStatus?.recent_discrepancies || []
  const unresolvedCount = discrepancies.filter((d) => !d.resolved).length

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold">Reconciliation</h1>
          <p className="text-text-muted text-sm">Database vs on-chain state verification</p>
        </div>
        <button
          onClick={handleTriggerReconciliation}
          disabled={reconStatus?.status === 'running' || triggerReconciliation.isPending}
          className="flex items-center gap-2 px-4 py-2 bg-shield hover:bg-shield-dark text-white rounded-lg transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
        >
          <Play className="w-4 h-4" />
          {triggerReconciliation.isPending ? 'Running...' : 'Run Reconciliation'}
        </button>
      </div>

      {/* Status Card */}
      <ReconciliationStatusCard status={reconStatus} isLoading={statusLoading} />

      {/* Summary Metrics */}
      <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
        <MetricCard
          label="Checked"
          value={reconStatus?.checked_count || 0}
          loading={statusLoading}
          icon="✓"
        />
        <MetricCard
          label="Discrepancies"
          value={reconStatus?.discrepancy_count || 0}
          loading={statusLoading}
          positive={(reconStatus?.discrepancy_count || 0) === 0}
          icon="⚠️"
        />
        <MetricCard
          label="Unresolved"
          value={unresolvedCount}
          loading={statusLoading}
          positive={unresolvedCount === 0}
          icon="🔴"
        />
        <MetricCard
          label="Duration"
          value={reconStatus?.duration_seconds ? `${reconStatus.duration_seconds.toFixed(1)}s` : '—'}
          loading={statusLoading}
          icon="⏱️"
        />
      </div>

      {/* Discrepancies */}
      <Card>
        <CardHeader>
          <div className="flex items-center justify-between">
            <CardTitle>Recent Discrepancies</CardTitle>
            <Badge variant={unresolvedCount > 0 ? 'danger' : 'success'}>
              {unresolvedCount} Unresolved
            </Badge>
          </div>
        </CardHeader>
        <CardContent>
          {statusLoading ? (
            <div className="text-center text-text-muted py-8">Loading discrepancies...</div>
          ) : discrepancies.length === 0 ? (
            <div className="text-center text-text-muted py-8">
              No discrepancies found. System is in sync.
            </div>
          ) : (
            <DiscrepanciesList discrepancies={discrepancies} />
          )}
        </CardContent>
      </Card>

      {/* History */}
      <Card>
        <CardHeader>
          <div className="flex items-center justify-between">
            <CardTitle>Reconciliation History</CardTitle>
            <select
              id="reconciliation-history-limit"
              name="historyLimit"
              value={historyLimit}
              onChange={(e) => setHistoryLimit(Number(e.target.value))}
              className="bg-surface-light border border-border rounded px-2 py-1 text-sm"
            >
              <option value="5">Last 5</option>
              <option value="10">Last 10</option>
              <option value="20">Last 20</option>
              <option value="50">Last 50</option>
            </select>
          </div>
        </CardHeader>
        <CardContent>
          {historyLoading ? (
            <div className="text-center text-text-muted py-8">Loading history...</div>
          ) : reconHistory ? (
            <ReconciliationHistory runs={reconHistory.runs} />
          ) : (
            <div className="text-center text-text-muted py-8">No history available</div>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
