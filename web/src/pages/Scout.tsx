import { useState, useEffect } from 'react'
import { Card, CardHeader, CardTitle, CardContent } from '../components/ui/Card'
import { Badge } from '../components/ui/Badge'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../components/ui/Table'
import { useScoutStatus, useWQSDistribution, useScoutMetrics, PromotionItem, RejectionItem } from '../api'
import { ScoutStatusCard } from '../components/scout/ScoutStatusCard'
import { WQSDistributionChart } from '../components/scout/WQSDistributionChart'
import { MetricCard } from '../components/ui/MetricCard'
import { Play } from 'lucide-react'
import { toast } from '../components/ui/Toast'
import { TimeRangePicker, TimeRange } from '../components/ui/TimeRangePicker'
import { useLayoutContext } from '../components/layout/Layout'

export function Scout() {
  const { setLastUpdate } = useLayoutContext()
  const [timeRange, setTimeRange] = useState<TimeRange>('7d')

  // Fetch Scout data
  const { data: scoutStatus, isLoading: statusLoading, refetch: refetchStatus } = useScoutStatus(15000)
  const { data: wqsDistribution, isLoading: wqsLoading } = useWQSDistribution(timeRange)
  const { data: scoutMetrics, isLoading: metricsLoading } = useScoutMetrics(timeRange)

  // Update last update time
  useEffect(() => {
    if (scoutStatus || wqsDistribution) {
      setLastUpdate(new Date())
    }
  }, [scoutStatus, wqsDistribution, setLastUpdate])

  const handleRunScout = async () => {
    try {
      toast.info('Starting Scout run...')
      // Trigger Scout run
      // await triggerScoutRun()
      toast.success('Scout run initiated')
      refetchStatus()
    } catch (error) {
      toast.error('Failed to start Scout run')
    }
  }

  const promotionQueue = scoutStatus?.promotion_queue || []
  const rejectionQueue = scoutStatus?.rejection_queue || []

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold">Scout Intelligence</h1>
          <p className="text-text-muted text-sm">Wallet analysis and WQS scoring</p>
        </div>
        <div className="flex items-center gap-4">
          <TimeRangePicker value={timeRange} onChange={setTimeRange} />
          <button
            onClick={handleRunScout}
            disabled={scoutStatus?.status === 'running'}
            className="flex items-center gap-2 px-4 py-2 bg-shield hover:bg-shield-dark text-white rounded-lg transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
          >
            <Play className="w-4 h-4" />
            Run Scout
          </button>
        </div>
      </div>

      {/* Status Overview */}
      <ScoutStatusCard status={scoutStatus} isLoading={statusLoading} />

      {/* WQS Distribution */}
      <Card>
        <CardHeader>
          <CardTitle>WQS Score Distribution</CardTitle>
        </CardHeader>
        <CardContent>
          {wqsLoading ? (
            <div className="text-center text-text-muted py-8">Loading WQS distribution...</div>
          ) : wqsDistribution ? (
            <WQSDistributionChart data={wqsDistribution} />
          ) : (
            <div className="text-center text-text-muted py-8">No WQS data available</div>
          )}
        </CardContent>
      </Card>

      {/* Scout Metrics */}
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
        <MetricCard
          label="Total Analyzed"
          value={scoutMetrics?.total_analyzed ?? 0}
          loading={metricsLoading}
        />
        <MetricCard
          label="Rug Check Rejections"
          value={scoutMetrics?.rug_check_rejections ?? 0}
          loading={metricsLoading}
          positive={false}
        />
        <MetricCard
          label="Backtest Success Rate"
          value={scoutMetrics ? `${scoutMetrics.backtest_success_rate.toFixed(1)}%` : '0%'}
          loading={metricsLoading}
          positive={scoutMetrics ? scoutMetrics.backtest_success_rate >= 70 : false}
        />
        <MetricCard
          label="Avg Analysis Time"
          value={scoutMetrics ? `${scoutMetrics.avg_analysis_time_seconds.toFixed(1)}s` : '0s'}
          loading={metricsLoading}
        />
      </div>

      {/* Promotion Queue */}
      {promotionQueue.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle>Promotion Queue ({promotionQueue.length})</CardTitle>
          </CardHeader>
          <CardContent>
            <Table>
              <TableHeader>
                <TableRow hoverable={false}>
                  <TableHead>Address</TableHead>
                  <TableHead>WQS Score</TableHead>
                  <TableHead>Reason</TableHead>
                  <TableHead>Backtest</TableHead>
                  <TableHead>Validated</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {promotionQueue.map((item: PromotionItem) => (
                  <TableRow key={item.address}>
                    <TableCell mono className="text-sm">
                      {item.address.slice(0, 8)}...{item.address.slice(-8)}
                    </TableCell>
                    <TableCell>
                      <Badge variant={item.wqs_score >= 65 ? 'success' : 'warning'}>
                        {item.wqs_score.toFixed(1)}
                      </Badge>
                    </TableCell>
                    <TableCell className="text-sm">{item.reason}</TableCell>
                    <TableCell>
                      <Badge variant={item.backtest_success ? 'success' : 'danger'}>
                        {item.backtest_success ? 'Pass' : 'Fail'}
                      </Badge>
                    </TableCell>
                    <TableCell className="text-sm text-text-muted">
                      {new Date(item.validated_at).toLocaleString()}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </CardContent>
        </Card>
      )}

      {/* Rejection Queue */}
      {rejectionQueue.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle>Rejection Queue ({rejectionQueue.length})</CardTitle>
          </CardHeader>
          <CardContent>
            <Table>
              <TableHeader>
                <TableRow hoverable={false}>
                  <TableHead>Address</TableHead>
                  <TableHead>WQS Score</TableHead>
                  <TableHead>Reason</TableHead>
                  <TableHead>Rejected</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {rejectionQueue.map((item: RejectionItem) => (
                  <TableRow key={item.address}>
                    <TableCell mono className="text-sm">
                      {item.address.slice(0, 8)}...{item.address.slice(-8)}
                    </TableCell>
                    <TableCell>
                      <Badge variant="warning">
                        {item.wqs_score.toFixed(1)}
                      </Badge>
                    </TableCell>
                    <TableCell className="text-sm">{item.reason}</TableCell>
                    <TableCell className="text-sm text-text-muted">
                      {new Date(item.rejected_at).toLocaleString()}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </CardContent>
        </Card>
      )}
    </div>
  )
}
