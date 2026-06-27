import { useState } from 'react'
import { RefreshCw, AlertTriangle, CheckCircle, XCircle, TrendingUp, Database, Zap } from 'lucide-react'
import { toast } from 'sonner'
import { RealTimeAlerts, ConnectionStatus } from '@/components/dashboard'
import { useWebSocket } from '@/hooks/useWebSocket'
import {
  useBudgetStatus,
  useCacheStats,
  useConvictionAllocation,
  triggerScoutRun
} from '@/api/scout'

export function ScoutDashboard() {
  const [isRunningScout, setIsRunningScout] = useState(false)

  // WebSocket integration
  const userToken = 'demo-token' // In production, get from auth store
  const { isConnected, isConnecting, connectionError } = useWebSocket({ apiKey: userToken })

  // Fetch Scout integration data
  const { data: budgetData, isLoading: budgetLoading, error: budgetError, refetch: refetchBudget } = useBudgetStatus()
  const { data: cacheData, isLoading: cacheLoading, error: cacheError, refetch: refetchCache } = useCacheStats()
  const { data: convictionData, isLoading: convictionLoading, error: convictionError, refetch: refetchConviction } = useConvictionAllocation()

  const isLoading = budgetLoading || cacheLoading || convictionLoading
  const hasErrors = budgetError || cacheError || convictionError

  const handleRunScout = async () => {
    setIsRunningScout(true)
    try {
      const result = await triggerScoutRun()
      toast.success('Scout run initiated', {
        description: `Run ID: ${result.run_id}`,
      })

      // Refetch all data after a delay
      setTimeout(() => {
        refetchBudget()
        refetchCache()
        refetchConviction()
      }, 5000)
    } catch (error) {
      toast.error('Failed to trigger Scout run', {
        description: error instanceof Error ? error.message : 'Unknown error',
      })
    } finally {
      setIsRunningScout(false)
    }
  }

  const getAlertLevelColor = (level: string) => {
    switch (level.toLowerCase()) {
      case 'depleted':
        return 'text-red-500 bg-red-500/10'
      case 'critical':
        return 'text-orange-500 bg-orange-500/10'
      case 'warning':
        return 'text-yellow-500 bg-yellow-500/10'
      default:
        return 'text-green-500 bg-green-500/10'
    }
  }

  const getAlertLevelIcon = (level: string) => {
    switch (level.toLowerCase()) {
      case 'depleted':
      case 'critical':
        return <AlertTriangle className="h-4 w-4" />
      case 'warning':
        return <AlertTriangle className="h-4 w-4" />
      default:
        return <CheckCircle className="h-4 w-4" />
    }
  }

  if (isLoading) {
    return (
      <div className="flex items-center justify-center min-h-screen">
        <div className="text-center">
          <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-shield mx-auto mb-4"></div>
          <p className="text-text-muted">Loading Scout integration data...</p>
        </div>
      </div>
    )
  }

  return (
    <div className="space-y-6 p-6">
      {/* Header */}
      <div className="flex justify-between items-center">
        <div>
          <h1 className="text-3xl font-bold text-text-primary">Scout Intelligence Dashboard</h1>
          <p className="text-text-muted mt-1">Real-time wallet analysis and integration metrics</p>
        </div>
        <button
          onClick={handleRunScout}
          disabled={isRunningScout}
          className="px-4 py-2 bg-shield text-white rounded-lg hover:bg-shield/90 disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-2"
        >
          <RefreshCw className={`h-4 w-4 ${isRunningScout ? 'animate-spin' : ''}`} />
          {isRunningScout ? 'Running Scout...' : 'Run Scout Analysis'}
        </button>
      </div>

      {/* Connection Status */}
      <ConnectionStatus
        isConnected={isConnected}
        isConnecting={isConnecting}
        connectionError={connectionError}
      />

      {/* Error Summary */}
      {hasErrors && (
        <div className="bg-red-500/10 border border-red-500/20 rounded-lg p-4">
          <div className="flex items-center gap-2 text-red-500">
            <XCircle className="h-5 w-5" />
            <span className="font-semibold">Data Loading Issues</span>
          </div>
          <p className="text-text-muted text-sm mt-1">
            Some Scout integration data failed to load. Please check your connection or try again.
          </p>
        </div>
      )}

      {/* Budget Management Section */}
      <div className="bg-surface border border-border rounded-lg p-6">
        <div className="flex items-center justify-between mb-6">
          <div className="flex items-center gap-3">
            <Database className="h-6 w-6 text-shield" />
            <h2 className="text-xl font-semibold">Predictive Budget Manager</h2>
          </div>
          <button onClick={() => refetchBudget()} className="text-text-muted hover:text-text-primary">
            <RefreshCw className="h-4 w-4" />
          </button>
        </div>

        {budgetData && (
          <div className="space-y-6">
            {/* Budget Overview */}
            <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
              <div className="bg-surface-light rounded-lg p-4">
                <p className="text-text-muted text-sm">Credits Used</p>
                <p className="text-2xl font-bold text-text-primary mt-1">
                  {budgetData.credits_used.toLocaleString()}
                </p>
              </div>
              <div className="bg-surface-light rounded-lg p-4">
                <p className="text-text-muted text-sm">Credits Remaining</p>
                <p className="text-2xl font-bold text-text-primary mt-1">
                  {budgetData.credits_remaining.toLocaleString()}
                </p>
              </div>
              <div className="bg-surface-light rounded-lg p-4">
                <p className="text-text-muted text-sm">Usage Percentage</p>
                <p className="text-2xl font-bold text-text-primary mt-1">
                  {budgetData.usage_percentage.toFixed(1)}%
                </p>
              </div>
              <div className={`bg-surface-light rounded-lg p-4 ${getAlertLevelColor(budgetData.alert_level)}`}>
                <p className="text-text-muted text-sm">Alert Level</p>
                <div className="flex items-center gap-2 mt-1">
                  {getAlertLevelIcon(budgetData.alert_level)}
                  <p className="text-2xl font-bold capitalize">{budgetData.alert_level}</p>
                </div>
              </div>
            </div>

            {/* Budget Forecast */}
            <div className="bg-surface-light rounded-lg p-4">
              <h3 className="font-semibold mb-3">24-Hour Forecast</h3>
              <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
                <div>
                  <p className="text-text-muted text-sm">Projected Usage</p>
                  <p className="text-lg font-semibold">{budgetData.forecast_24h.projected_usage.toLocaleString()} credits</p>
                </div>
                <div>
                  <p className="text-text-muted text-sm">Projected Remaining</p>
                  <p className="text-lg font-semibold">{budgetData.forecast_24h.projected_remaining.toLocaleString()} credits</p>
                </div>
                <div>
                  <p className="text-text-muted text-sm">Confidence</p>
                  <p className="text-lg font-semibold">{(budgetData.forecast_24h.confidence * 100).toFixed(0)}%</p>
                </div>
              </div>
              <div className="mt-3">
                <p className="text-text-muted text-sm">Trend: <span className="font-semibold capitalize">{budgetData.forecast_24h.trend}</span></p>
                <div className="mt-2">
                  <p className="text-sm font-semibold mb-1">Recommendations:</p>
                  <ul className="text-sm text-text-muted list-disc list-inside">
                    {budgetData.forecast_24h.recommendations.map((rec, idx) => (
                      <li key={idx}>{rec}</li>
                    ))}
                  </ul>
                </div>
              </div>
            </div>

            {/* Optimization Suggestions */}
            {budgetData.optimization_suggestions.length > 0 && (
              <div className="bg-surface-light rounded-lg p-4">
                <h3 className="font-semibold mb-3">Optimization Suggestions</h3>
                <div className="space-y-2">
                  {budgetData.optimization_suggestions.map((suggestion, idx) => (
                    <div key={idx} className="flex items-center justify-between p-3 bg-surface rounded">
                      <div className="flex-1">
                        <p className="font-medium">{suggestion.description}</p>
                        <p className="text-sm text-text-muted">Action: {suggestion.action_type}</p>
                      </div>
                      <div className="text-right">
                        <p className="text-sm font-semibold">Save {suggestion.expected_savings.toLocaleString()} credits</p>
                        <p className="text-xs text-text-muted capitalize">{suggestion.priority} priority</p>
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>
        )}
      </div>

      {/* Cache Management Section */}
      <div className="bg-surface border border-border rounded-lg p-6">
        <div className="flex items-center justify-between mb-6">
          <div className="flex items-center gap-3">
            <Zap className="h-6 w-6 text-spear" />
            <h2 className="text-xl font-semibold">Activity-Based Cache</h2>
          </div>
          <button onClick={() => refetchCache()} className="text-text-muted hover:text-text-primary">
            <RefreshCw className="h-4 w-4" />
          </button>
        </div>

        {cacheData && (
          <div className="space-y-6">
            {/* Cache Overview */}
            <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
              <div className="bg-surface-light rounded-lg p-4">
                <p className="text-text-muted text-sm">Hit Rate</p>
                <p className="text-2xl font-bold text-green-500 mt-1">{cacheData.hit_rate.toFixed(1)}%</p>
              </div>
              <div className="bg-surface-light rounded-lg p-4">
                <p className="text-text-muted text-sm">Miss Rate</p>
                <p className="text-2xl font-bold text-orange-500 mt-1">{cacheData.miss_rate.toFixed(1)}%</p>
              </div>
              <div className="bg-surface-light rounded-lg p-4">
                <p className="text-text-muted text-sm">Total Entries</p>
                <p className="text-2xl font-bold text-text-primary mt-1">{cacheData.total_entries.toLocaleString()}</p>
              </div>
              <div className="bg-surface-light rounded-lg p-4">
                <p className="text-text-muted text-sm">Cache Efficiency</p>
                <p className="text-2xl font-bold text-shield mt-1">{cacheData.cache_efficiency.toFixed(1)}%</p>
              </div>
            </div>

            {/* Activity Distribution */}
            <div className="bg-surface-light rounded-lg p-4">
              <h3 className="font-semibold mb-3">Activity Distribution</h3>
              <div className="grid grid-cols-1 md:grid-cols-5 gap-4">
                <div className="bg-green-500/10 rounded p-3">
                  <p className="text-sm text-text-muted">Very High</p>
                  <p className="text-xl font-bold text-green-500">{cacheData.activity_distribution.very_high}</p>
                </div>
                <div className="bg-blue-500/10 rounded p-3">
                  <p className="text-sm text-text-muted">High</p>
                  <p className="text-xl font-bold text-blue-500">{cacheData.activity_distribution.high}</p>
                </div>
                <div className="bg-yellow-500/10 rounded p-3">
                  <p className="text-sm text-text-muted">Medium</p>
                  <p className="text-xl font-bold text-yellow-500">{cacheData.activity_distribution.medium}</p>
                </div>
                <div className="bg-orange-500/10 rounded p-3">
                  <p className="text-sm text-text-muted">Low</p>
                  <p className="text-xl font-bold text-orange-500">{cacheData.activity_distribution.low}</p>
                </div>
                <div className="bg-gray-500/10 rounded p-3">
                  <p className="text-sm text-text-muted">Inactive</p>
                  <p className="text-xl font-bold text-gray-500">{cacheData.activity_distribution.inactive}</p>
                </div>
              </div>
            </div>

            {/* Cache Statistics */}
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              <div className="bg-surface-light rounded-lg p-4">
                <h3 className="font-semibold mb-2">Cache Performance</h3>
                <div className="space-y-2">
                  <div className="flex justify-between">
                    <span className="text-text-muted">Total Hits:</span>
                    <span className="font-semibold">{cacheData.total_hits.toLocaleString()}</span>
                  </div>
                  <div className="flex justify-between">
                    <span className="text-text-muted">Total Misses:</span>
                    <span className="font-semibold">{cacheData.total_misses.toLocaleString()}</span>
                  </div>
                  <div className="flex justify-between">
                    <span className="text-text-muted">Max Size:</span>
                    <span className="font-semibold">{cacheData.max_size.toLocaleString()}</span>
                  </div>
                </div>
              </div>
              <div className="bg-surface-light rounded-lg p-4">
                <h3 className="font-semibold mb-2">Hit Rate Trend</h3>
                <div className="space-y-2">
                  <div className="flex justify-between">
                    <span className="text-text-muted">Current Hit Rate:</span>
                    <span className="font-semibold text-green-500">{cacheData.hit_rate.toFixed(1)}%</span>
                  </div>
                  <div className="flex justify-between">
                    <span className="text-text-muted">Efficiency Score:</span>
                    <span className="font-semibold text-shield">{cacheData.cache_efficiency.toFixed(1)}%</span>
                  </div>
                  <div className="flex justify-between">
                    <span className="text-text-muted">Cache Usage:</span>
                    <span className="font-semibold">
                      {((cacheData.total_entries / cacheData.max_size) * 100).toFixed(1)}%
                    </span>
                  </div>
                </div>
              </div>
            </div>
          </div>
        )}
      </div>

      {/* High Conviction Allocator Section */}
      <div className="bg-surface border border-border rounded-lg p-6">
        <div className="flex items-center justify-between mb-6">
          <div className="flex items-center gap-3">
            <TrendingUp className="h-6 w-6 text-shield" />
            <h2 className="text-xl font-semibold">High Conviction Allocator</h2>
          </div>
          <button onClick={() => refetchConviction()} className="text-text-muted hover:text-text-primary">
            <RefreshCw className="h-4 w-4" />
          </button>
        </div>

        {convictionData && (
          <div className="space-y-6">
            {/* Overview */}
            <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
              <div className="bg-surface-light rounded-lg p-4">
                <p className="text-text-muted text-sm">Total Wallets Analyzed</p>
                <p className="text-2xl font-bold text-text-primary mt-1">{convictionData.total_wallets_analyzed}</p>
              </div>
              <div className="bg-surface-light rounded-lg p-4">
                <p className="text-text-muted text-sm">High Conviction Count</p>
                <p className="text-2xl font-bold text-shield mt-1">{convictionData.high_conviction_count}</p>
              </div>
              <div className="bg-surface-light rounded-lg p-4">
                <p className="text-text-muted text-sm">Total Credits Allocated</p>
                <p className="text-2xl font-bold text-spear mt-1">{convictionData.allocation_summary.total_credits_allocated.toLocaleString()}</p>
              </div>
            </div>

            {/* Budget Allocation */}
            <div className="bg-surface-light rounded-lg p-4">
              <h3 className="font-semibold mb-3">Budget Allocation</h3>
              <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
                <div className="bg-shield/10 rounded p-3">
                  <p className="text-sm text-text-muted">High Conviction (70%)</p>
                  <p className="text-xl font-bold text-shield">{convictionData.budget_remaining.high_conviction.toLocaleString()}</p>
                </div>
                <div className="bg-spear/10 rounded p-3">
                  <p className="text-sm text-text-muted">Emerging (20%)</p>
                  <p className="text-xl font-bold text-spear">{convictionData.budget_remaining.emerging.toLocaleString()}</p>
                </div>
                <div className="bg-gray-500/10 rounded p-3">
                  <p className="text-sm text-text-muted">Reserve (10%)</p>
                  <p className="text-xl font-bold text-gray-500">{convictionData.budget_remaining.reserve.toLocaleString()}</p>
                </div>
              </div>
            </div>

            {/* Wallet Analysis Breakdown */}
            <div className="bg-surface-light rounded-lg p-4">
              <h3 className="font-semibold mb-3">Wallet Analysis Breakdown</h3>
              <div className="overflow-x-auto">
                <table className="w-full text-sm">
                  <thead>
                    <tr className="border-b border-border">
                      <th className="text-left p-2">Conviction Level</th>
                      <th className="text-right p-2">Count</th>
                      <th className="text-right p-2">Credits Used</th>
                      <th className="text-right p-2">Avg WQS</th>
                      <th className="text-right p-2">ROI Score</th>
                    </tr>
                  </thead>
                  <tbody>
                    <tr className="border-b border-border/50">
                      <td className="p-2 font-medium">Very High (80+)</td>
                      <td className="text-right p-2">{convictionData.wallets_analyzed.very_high.count}</td>
                      <td className="text-right p-2">{convictionData.wallets_analyzed.very_high.credits_used.toLocaleString()}</td>
                      <td className="text-right p-2">{convictionData.wallets_analyzed.very_high.average_wqs.toFixed(1)}</td>
                      <td className="text-right p-2 text-green-500">{(convictionData.wallets_analyzed.very_high.roi_score * 100).toFixed(0)}%</td>
                    </tr>
                    <tr className="border-b border-border/50">
                      <td className="p-2 font-medium">High (70-79)</td>
                      <td className="text-right p-2">{convictionData.wallets_analyzed.high.count}</td>
                      <td className="text-right p-2">{convictionData.wallets_analyzed.high.credits_used.toLocaleString()}</td>
                      <td className="text-right p-2">{convictionData.wallets_analyzed.high.average_wqs.toFixed(1)}</td>
                      <td className="text-right p-2 text-green-500">{(convictionData.wallets_analyzed.high.roi_score * 100).toFixed(0)}%</td>
                    </tr>
                    <tr className="border-b border-border/50">
                      <td className="p-2 font-medium">Medium (50-69)</td>
                      <td className="text-right p-2">{convictionData.wallets_analyzed.medium.count}</td>
                      <td className="text-right p-2">{convictionData.wallets_analyzed.medium.credits_used.toLocaleString()}</td>
                      <td className="text-right p-2">{convictionData.wallets_analyzed.medium.average_wqs.toFixed(1)}</td>
                      <td className="text-right p-2 text-yellow-500">{(convictionData.wallets_analyzed.medium.roi_score * 100).toFixed(0)}%</td>
                    </tr>
                    <tr className="border-b border-border/50">
                      <td className="p-2 font-medium">Emerging (30-49)</td>
                      <td className="text-right p-2">{convictionData.wallets_analyzed.emerging.count}</td>
                      <td className="text-right p-2">{convictionData.wallets_analyzed.emerging.credits_used.toLocaleString()}</td>
                      <td className="text-right p-2">{convictionData.wallets_analyzed.emerging.average_wqs.toFixed(1)}</td>
                      <td className="text-right p-2 text-orange-500">{(convictionData.wallets_analyzed.emerging.roi_score * 100).toFixed(0)}%</td>
                    </tr>
                    <tr>
                      <td className="p-2 font-medium">Low (&lt;30)</td>
                      <td className="text-right p-2">{convictionData.wallets_analyzed.low.count}</td>
                      <td className="text-right p-2">{convictionData.wallets_analyzed.low.credits_used.toLocaleString()}</td>
                      <td className="text-right p-2">{convictionData.wallets_analyzed.low.average_wqs.toFixed(1)}</td>
                      <td className="text-right p-2 text-red-500">{(convictionData.wallets_analyzed.low.roi_score * 100).toFixed(0)}%</td>
                    </tr>
                  </tbody>
                </table>
              </div>
            </div>

            {/* Allocation Summary */}
            <div className="bg-surface-light rounded-lg p-4">
              <h3 className="font-semibold mb-3">Allocation Summary</h3>
              <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
                <div>
                  <p className="text-text-muted text-sm">High Conviction %</p>
                  <p className="text-lg font-semibold">{convictionData.allocation_summary.high_conviction_percentage.toFixed(0)}%</p>
                </div>
                <div>
                  <p className="text-text-muted text-sm">Emerging %</p>
                  <p className="text-lg font-semibold">{convictionData.allocation_summary.emerging_percentage.toFixed(0)}%</p>
                </div>
                <div>
                  <p className="text-text-muted text-sm">Avg Credits/Wallet</p>
                  <p className="text-lg font-semibold">{convictionData.allocation_summary.average_credits_per_wallet.toFixed(0)}</p>
                </div>
              </div>
            </div>
          </div>
        )}
      </div>

      {/* Real-time Alerts */}
      <RealTimeAlerts />
    </div>
  )
}
