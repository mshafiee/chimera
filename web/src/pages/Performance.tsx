import { Card, CardHeader, CardTitle, CardContent } from '../components/ui/Card'
import { Badge } from '../components/ui/Badge'
import { useTradeLatency, useRPCLatency, useDatabasePerformance, useRequestRate, useCostAnalysis } from '../api'
import { LatencyChart } from '../components/performance/LatencyChart'
import { RPCLatencyTable } from '../components/performance/RPCLatencyTable'
import { DatabasePerformanceCard } from '../components/performance/DatabasePerformanceCard'
import { RequestRateCard } from '../components/performance/RequestRateCard'
import { CostAnalysisChart } from '../components/performance/CostAnalysisChart'
import { MetricCard } from '../components/ui/MetricCard'
import { TimeRangePicker, TimeRange } from '../components/ui/TimeRangePicker'
import { useState } from 'react'

export function Performance() {
  const [timeRange, setTimeRange] = useState<TimeRange>('24h')

  const { data: tradeLatency, isLoading: latencyLoading } = useTradeLatency(timeRange)
  const { data: rpcLatency, isLoading: rpcLoading } = useRPCLatency()
  const { data: dbPerformance, isLoading: dbLoading } = useDatabasePerformance()
  const { data: requestRate, isLoading: rateLoading } = useRequestRate()
  const { data: costAnalysis, isLoading: costLoading } = useCostAnalysis(timeRange)

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold">Performance Analytics</h1>
          <p className="text-text-muted text-sm">System performance and cost analysis</p>
        </div>
        <TimeRangePicker value={timeRange} onChange={setTimeRange} />
      </div>

      {/* Trade Latency */}
      <Card>
        <CardHeader>
          <CardTitle>Trade Execution Latency</CardTitle>
        </CardHeader>
        <CardContent>
          {latencyLoading ? (
            <div className="text-center text-text-muted py-8">Loading latency data...</div>
          ) : tradeLatency ? (
            <div className="space-y-6">
              {/* Summary */}
              <div className="grid grid-cols-4 gap-4">
                <div className="bg-surface-light rounded-lg p-4">
                  <div className="text-xs text-text-muted mb-1">p50</div>
                  <div className="text-xl font-semibold font-mono-numbers">
                    {tradeLatency.p50.toFixed(0)}ms
                  </div>
                </div>
                <div className="bg-surface-light rounded-lg p-4">
                  <div className="text-xs text-text-muted mb-1">p95</div>
                  <div className="text-xl font-semibold font-mono-numbers">
                    {tradeLatency.p95.toFixed(0)}ms
                  </div>
                </div>
                <div className="bg-surface-light rounded-lg p-4">
                  <div className="text-xs text-text-muted mb-1">p99</div>
                  <div className="text-xl font-semibold font-mono-numbers">
                    {tradeLatency.p99.toFixed(0)}ms
                  </div>
                </div>
                <div className="bg-surface-light rounded-lg p-4">
                  <div className="text-xs text-text-muted mb-1">Avg</div>
                  <div className="text-xl font-semibold font-mono-numbers">
                    {tradeLatency.avg.toFixed(0)}ms
                  </div>
                </div>
              </div>
              <LatencyChart data={tradeLatency} />
            </div>
          ) : (
            <div className="text-center text-text-muted py-8">No latency data available</div>
          )}
        </CardContent>
      </Card>

      {/* RPC Latency */}
      <Card>
        <CardHeader>
          <CardTitle>RPC Endpoint Latency</CardTitle>
        </CardHeader>
        <CardContent>
          {rpcLoading ? (
            <div className="text-center text-text-muted py-8">Loading RPC data...</div>
          ) : rpcLatency ? (
            <RPCLatencyTable data={rpcLatency} />
          ) : (
            <div className="text-center text-text-muted py-8">No RPC data available</div>
          )}
        </CardContent>
      </Card>

      {/* Database Performance */}
      <Card>
        <CardHeader>
          <CardTitle>Database Performance</CardTitle>
        </CardHeader>
        <CardContent>
          {dbLoading ? (
            <div className="text-center text-text-muted py-8">Loading database data...</div>
          ) : dbPerformance ? (
            <DatabasePerformanceCard data={dbPerformance} />
          ) : (
            <div className="text-center text-text-muted py-8">No database data available</div>
          )}
        </CardContent>
      </Card>

      {/* Request Rate */}
      <Card>
        <CardHeader>
          <CardTitle>Request Rate</CardTitle>
        </CardHeader>
        <CardContent>
          {rateLoading ? (
            <div className="text-center text-text-muted py-8">Loading request rate...</div>
          ) : requestRate ? (
            <RequestRateCard data={requestRate} />
          ) : (
            <div className="text-center text-text-muted py-8">No request rate data available</div>
          )}
        </CardContent>
      </Card>

      {/* Cost Analysis */}
      <Card>
        <CardHeader>
          <CardTitle>Cost Analysis (Per-Trade Breakdown)</CardTitle>
        </CardHeader>
        <CardContent>
          {costLoading ? (
            <div className="text-center text-text-muted py-8">Loading cost data...</div>
          ) : costAnalysis ? (
            <CostAnalysisChart data={costAnalysis} />
          ) : (
            <div className="text-center text-text-muted py-8">No cost data available</div>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
