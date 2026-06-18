import type { DatabasePerformanceResponse } from '../../api'
import { MetricCard } from '../ui/MetricCard'

interface DatabasePerformanceCardProps {
  data: DatabasePerformanceResponse
}

export function DatabasePerformanceCard({ data }: DatabasePerformanceCardProps) {
  return (
    <div className="space-y-6">
      {/* Query Latency */}
      <div>
        <h3 className="text-sm font-medium mb-3">Query Latency</h3>
        <div className="grid grid-cols-4 gap-4">
          <MetricCard
            label="Avg"
            value={`${data.query_latency.avg_ms.toFixed(0)}ms`}
            size="sm"
          />
          <MetricCard
            label="p95"
            value={`${data.query_latency.p95_ms.toFixed(0)}ms`}
            size="sm"
          />
          <MetricCard
            label="p99"
            value={`${data.query_latency.p99_ms.toFixed(0)}ms`}
            size="sm"
          />
          <MetricCard
            label="Slow Queries"
            value={data.query_latency.slow_queries}
            positive={data.query_latency.slow_queries === 0}
            size="sm"
          />
        </div>
      </div>

      {/* Connection Pool */}
      <div>
        <h3 className="text-sm font-medium mb-3">Connection Pool</h3>
        <div className="grid grid-cols-4 gap-4">
          <MetricCard
            label="Active"
            value={data.connection_pool.active_connections}
            size="sm"
          />
          <MetricCard
            label="Idle"
            value={data.connection_pool.idle_connections}
            size="sm"
          />
          <MetricCard
            label="Max"
            value={data.connection_pool.max_connections}
            size="sm"
          />
          <MetricCard
            label="Utilization"
            value={`${data.connection_pool.utilization_percent.toFixed(0)}%`}
            positive={data.connection_pool.utilization_percent < 80}
            size="sm"
          />
        </div>
      </div>

      {/* Cache Performance */}
      <div>
        <h3 className="text-sm font-medium mb-3">Cache Performance</h3>
        <div className="grid grid-cols-4 gap-4">
          <MetricCard
            label="Hit Rate"
            value={`${(data.cache_performance.hit_rate * 100).toFixed(1)}%`}
            positive={data.cache_performance.hit_rate > 0.8}
            size="sm"
          />
          <MetricCard
            label="Miss Rate"
            value={`${(data.cache_performance.miss_rate * 100).toFixed(1)}%`}
            positive={data.cache_performance.miss_rate < 0.2}
            size="sm"
          />
          <MetricCard
            label="Size"
            value={`${data.cache_performance.size}/${data.cache_performance.max_size}`}
            size="sm"
          />
          <MetricCard
            label="Total Hits"
            value={data.cache_performance.total_hits.toLocaleString()}
            size="sm"
          />
        </div>
      </div>
    </div>
  )
}
