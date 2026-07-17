import { useState } from 'react'
import { TimeRangePicker } from '@/components/ui/TimeRangePicker'
import { PortfolioHeatChart } from '@/components/charts/PortfolioHeatChart'
import { ConcentrationRiskChart } from '@/components/charts/ConcentrationRiskChart'
import { DrawdownChart } from '@/components/charts/DrawdownChart'
import { StopLossProfitChart } from '@/components/charts/StopLossProfitChart'
import { RealTimeAlerts, ConnectionStatus } from '@/components/dashboard'
import { useDashboardWebSocket } from '@/hooks/useDashboardWebSocket'
import { useWebSocket } from '@/hooks/useWebSocket'
import { useAuthStore } from '../stores/authStore'
import {
  usePortfolioRisk,
  useStopLossMetrics,
  useProfitTargetMetrics,
  usePositionSizeAnalysis
} from '@/api/risk'
import type { TimeRange } from '@/components/ui/TimeRangePicker'

export function RiskDashboard() {
  const [timeRange, setTimeRange] = useState<TimeRange>('24h')

  // WebSocket integration
  const userToken = useAuthStore(state => state.user?.token) ?? ''
  const { isConnected, isConnecting, connectionError } = useWebSocket({ apiKey: userToken })
  const { refreshRiskData } = useDashboardWebSocket({
    onRiskUpdate: (data) => {
      console.log('Risk update received:', data)
    },
    onHeatAlert: (data) => {
      console.log('Heat alert received:', data)
    },
  })

  // Fetch data from API
  const { data: portfolioRisk, isLoading: portfolioLoading } = usePortfolioRisk()
  const { data: stopLossMetrics, isLoading: stopLossLoading } = useStopLossMetrics(timeRange)
  const { data: profitTargetMetrics, isLoading: profitTargetLoading } = useProfitTargetMetrics(timeRange)
  const { data: positionSizeAnalysis, isLoading: positionSizeLoading } = usePositionSizeAnalysis()

  const isLoading = portfolioLoading || stopLossLoading || profitTargetLoading || positionSizeLoading

  if (isLoading) {
    return (
      <div className="flex items-center justify-center min-h-screen">
        <div className="text-center">
          <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-shield mx-auto mb-4"></div>
          <p className="text-text-muted">Loading risk analysis...</p>
        </div>
      </div>
    )
  }

  if (!portfolioRisk) {
    return (
      <div className="flex items-center justify-center min-h-screen">
        <div className="text-center">
          <p className="text-red-600 mb-2">Failed to load risk data</p>
          <p className="text-text-muted text-sm">Please try again later</p>
        </div>
      </div>
    )
  }

  return (
    <div className="container mx-auto px-4 py-8">
      {/* Real-time components */}
      <RealTimeAlerts maxAlerts={3} />

      {/* Header */}
      <div className="mb-8 flex items-center justify-between">
        <div>
          <h1 className="text-3xl font-bold text-gray-900 mb-2">Risk Analysis Dashboard</h1>
          <p className="text-gray-600">Real-time portfolio risk monitoring and performance metrics</p>
        </div>
        <ConnectionStatus
          isConnected={isConnected}
          isConnecting={isConnecting}
          connectionError={connectionError}
        />
      </div>

      {/* Time Range Selector */}
      <div className="mb-6 flex items-center gap-4">
        <TimeRangePicker value={timeRange} onChange={setTimeRange} />
        <button
          onClick={refreshRiskData}
          className="px-4 py-2 text-sm font-medium text-white bg-shield rounded-lg hover:bg-shield/90 transition-colors"
        >
          Refresh Data
        </button>
      </div>

      {/* Dashboard Grid */}
      <div className="space-y-6">
        {/* First Row: Portfolio Heat & Concentration */}
        <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
          <PortfolioHeatChart
            heatPercentage={portfolioRisk.portfolio_heat_percent}
            heatThreshold={portfolioRisk.heat_threshold}
            heatStatus={portfolioRisk.heat_status === 'high' ? 'elevated' : portfolioRisk.heat_status}
          />

          <ConcentrationRiskChart
            byToken={portfolioRisk.concentration.by_token.map(token => ({
              name: token.token_symbol || 'Unknown',
              value: token.total_value_sol,
              percentage: token.percentage
            }))}
            bySector={portfolioRisk.concentration.by_sector.map(sector => ({
              name: sector.sector,
              value: sector.total_value_sol,
              percentage: sector.percentage
            }))}
            maxConcentrationPercent={portfolioRisk.concentration.max_concentration_percent}
            hhi={portfolioRisk.concentration.hhi}
          />
        </div>

        {/* Second Row: Drawdown Analysis */}
        <div>
          <DrawdownChart
            currentDrawdownPercent={portfolioRisk.drawdown.current_drawdown_percent}
            maxDrawdownPercent={portfolioRisk.drawdown.max_drawdown_percent}
            drawdownDurationDays={portfolioRisk.drawdown.drawdown_duration_days}
            recoveryPercent={portfolioRisk.drawdown.recovery_percent}
          />
        </div>

        {/* Third Row: Stop Loss & Profit Targets */}
        {stopLossMetrics && profitTargetMetrics && (
          <div>
            <StopLossProfitChart
              activationRate={stopLossMetrics.activation_rate}
              totalActivations={stopLossMetrics.total_activations}
              lossPreventedSol={stopLossMetrics.loss_prevented_sol}
              averageLossPreventedSol={stopLossMetrics.average_loss_prevented_sol}
              activationsByStrategy={stopLossMetrics.activations_by_strategy.map(sl => ({
                strategy: sl.strategy,
                activations: sl.activations,
                lossPrevented: sl.loss_prevented_sol,
                averageLoss: sl.loss_prevented_sol / sl.activations
              }))}
              hitRate={profitTargetMetrics.hit_rate}
              totalHits={profitTargetMetrics.total_hits}
              totalTargets={profitTargetMetrics.total_targets}
              trailingStopActivations={profitTargetMetrics.trailing_stop_activations}
              averageRealizedGainSol={profitTargetMetrics.average_realized_gain_sol}
              targetsByStrategy={profitTargetMetrics.targets_by_strategy.map(pt => ({
                strategy: pt.strategy,
                hitRate: pt.hit_rate,
                totalHits: pt.total_hits,
                averageGain: pt.average_gain_sol
              }))}
              recentHits={profitTargetMetrics.recent_hits.map(hit => ({
                timestamp: hit.timestamp,
                token: hit.token_symbol || 'Unknown',
                gain: hit.realized_gain_sol
              }))}
            />
          </div>
        )}

        {/* Fourth Row: Position Size Analysis */}
        {positionSizeAnalysis && (
          <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
            <div className="p-6 bg-white rounded-lg shadow">
              <h3 className="text-lg font-semibold text-gray-900 mb-4">Position Size Statistics</h3>
              <div className="space-y-4">
                <div className="flex justify-between items-center">
                  <span className="text-gray-600">Average Position</span>
                  <span className="text-xl font-bold text-shield">
                    {positionSizeAnalysis.average_position_sol.toFixed(2)} SOL
                  </span>
                </div>
                <div className="flex justify-between items-center">
                  <span className="text-gray-600">Median Position</span>
                  <span className="text-xl font-bold text-shield">
                    {positionSizeAnalysis.median_position_sol.toFixed(2)} SOL
                  </span>
                </div>
                <div className="flex justify-between items-center">
                  <span className="text-gray-600">Max Position</span>
                  <span className="text-xl font-bold text-shield">
                    {positionSizeAnalysis.max_position_sol.toFixed(2)} SOL
                  </span>
                </div>
                <div className="flex justify-between items-center">
                  <span className="text-gray-600">Min Position</span>
                  <span className="text-xl font-bold text-shield">
                    {positionSizeAnalysis.min_position_sol.toFixed(2)} SOL
                  </span>
                </div>
              </div>
            </div>

            <div className="lg:col-span-2 p-6 bg-white rounded-lg shadow">
              <h3 className="text-lg font-semibold text-gray-900 mb-4">Position Size Distribution</h3>
              <div className="space-y-3">
                {positionSizeAnalysis.position_size_distribution.map((bucket) => (
                  <div key={bucket.range} className="flex items-center">
                    <div className="w-24 text-sm text-gray-600">{bucket.range}</div>
                    <div className="flex-1 mx-4">
                      <div className="w-full bg-gray-200 rounded-full h-6">
                        <div
                          className="bg-shield h-6 rounded-full flex items-center justify-end pr-2"
                          style={{ width: `${bucket.percentage}%` }}
                        >
                          <span className="text-xs text-white font-medium">{bucket.count}</span>
                        </div>
                      </div>
                    </div>
                    <div className="w-16 text-right text-sm text-gray-600">
                      {bucket.percentage.toFixed(0)}%
                    </div>
                  </div>
                ))}
              </div>
            </div>
          </div>
        )}

        {/* Kelly Criterion Usage */}
        {positionSizeAnalysis && (
          <div className="grid grid-cols-1 md:grid-cols-3 gap-6">
            <div className="p-6 bg-gradient-to-br from-shield/10 to-shield/5 rounded-lg shadow border border-shield/20">
              <h3 className="text-sm font-medium text-gray-600 mb-2">Kelly Criterion Usage</h3>
              <p className="text-3xl font-bold text-shield">
                {(positionSizeAnalysis.kelly_criterion_usage * 100).toFixed(0)}%
              </p>
              <p className="text-xs text-gray-500 mt-1">Optimal position sizing alignment</p>
            </div>

            <div className="p-6 bg-gradient-to-br from-blue-50 to-blue-100 rounded-lg shadow">
              <h3 className="text-sm font-medium text-gray-600 mb-2">Position Diversity</h3>
              <p className="text-3xl font-bold text-blue-600">
                {positionSizeAnalysis.position_size_distribution.length}
              </p>
              <p className="text-xs text-gray-500 mt-1">Size categories</p>
            </div>

            <div className="p-6 bg-gradient-to-br from-purple-50 to-purple-100 rounded-lg shadow">
              <h3 className="text-sm font-medium text-gray-600 mb-2">Total Positions</h3>
              <p className="text-3xl font-bold text-purple-600">
                {positionSizeAnalysis.position_size_distribution.reduce((sum, bucket) => sum + bucket.count, 0)}
              </p>
              <p className="text-xs text-gray-500 mt-1">Active positions</p>
            </div>
          </div>
        )}
      </div>

      {/* Footer */}
      <div className="mt-8 text-center text-sm text-gray-500">
        <p>Last updated: {new Date().toLocaleString()}</p>
        <p className="mt-1">Data refreshes every 15 seconds</p>
      </div>
    </div>
  )
}