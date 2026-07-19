import { useEffect, useState, type ReactNode } from 'react'
import { RefreshCw } from 'lucide-react'
import { TimeRangePicker } from '@/components/ui/TimeRangePicker'
import { PortfolioHeatChart } from '@/components/charts/PortfolioHeatChart'
import { ConcentrationRiskChart } from '@/components/charts/ConcentrationRiskChart'
import { DrawdownChart } from '@/components/charts/DrawdownChart'
import { StopLossProfitChart } from '@/components/charts/StopLossProfitChart'
import { RealTimeAlerts, ConnectionStatus } from '@/components/dashboard'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card'
import { Badge } from '@/components/ui/Badge'
import { MetricCard } from '@/components/ui/MetricCard'
import { ApiErrorBanner } from '@/components/ui/ApiErrorBanner'
import { toast } from '@/components/ui/Toast'
import { useLayoutContext } from '@/components/layout/Layout'
import { useDashboardWebSocket } from '@/hooks/useDashboardWebSocket'
import { useWebSocket } from '@/hooks/useWebSocket'
import { useAuthStore } from '../stores/authStore'
import {
  usePortfolioRisk,
  useStopLossMetrics,
  useProfitTargetMetrics,
  usePositionSizeAnalysis,
} from '@/api/risk'
import { safeToFixed } from '@/lib/format'
import type { TimeRange } from '@/components/ui/TimeRangePicker'

type HeatStatus = 'normal' | 'elevated' | 'high' | 'critical'

function heatValueClass(status: HeatStatus): string {
  switch (status) {
    case 'critical':
    case 'high':
      return 'text-loss'
    case 'elevated':
      return 'text-spear'
    default:
      return 'text-profit'
  }
}

function heatBadgeVariant(status: HeatStatus): 'danger' | 'warning' | 'success' {
  switch (status) {
    case 'critical':
    case 'high':
      return 'danger'
    case 'elevated':
      return 'warning'
    default:
      return 'success'
  }
}

function concentrationClass(percent: number): string {
  if (percent > 30) return 'text-loss'
  if (percent > 15) return 'text-spear'
  return 'text-profit'
}

export function RiskDashboard() {
  const { setLastUpdate } = useLayoutContext()
  const [timeRange, setTimeRange] = useState<TimeRange>('24h')

  // WebSocket integration
  const userToken = useAuthStore((state) => state.user?.token) ?? ''
  const { isConnected, isConnecting, connectionError } = useWebSocket({ apiKey: userToken })
  const { refreshRiskData } = useDashboardWebSocket({
    onHeatAlert: (data) => {
      const message = data.message || 'Portfolio heat alert'
      if (data.severity === 'high') toast.error(message, 10000)
      else if (data.severity === 'medium') toast.warning(message)
      else toast.info(message)
    },
  })

  // Fetch data from API
  const {
    data: portfolioRisk,
    isLoading: portfolioLoading,
    error: portfolioError,
  } = usePortfolioRisk()
  const {
    data: stopLossMetrics,
    isLoading: stopLossLoading,
    error: stopLossError,
  } = useStopLossMetrics(timeRange)
  const {
    data: profitTargetMetrics,
    isLoading: profitTargetLoading,
    error: profitTargetError,
  } = useProfitTargetMetrics(timeRange)
  const {
    data: positionSizeAnalysis,
    isLoading: positionSizeLoading,
    error: positionSizeError,
  } = usePositionSizeAnalysis()

  useEffect(() => {
    if (portfolioRisk) setLastUpdate(new Date())
  }, [portfolioRisk, setLastUpdate])

  const heatStatus: HeatStatus = portfolioRisk?.heat_status ?? 'normal'

  return (
    <div className="space-y-6">
      <ApiErrorBanner
        errors={[portfolioError, stopLossError, profitTargetError, positionSizeError]}
      />

      <RealTimeAlerts maxAlerts={3} />

      {/* Header */}
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-2xl md:text-3xl font-bold text-text">Risk Analysis Dashboard</h1>
          <p className="text-text-muted text-sm md:text-base mt-1">
            Real-time portfolio risk monitoring and performance metrics
          </p>
        </div>
        <ConnectionStatus
          isConnected={isConnected}
          isConnecting={isConnecting}
          connectionError={connectionError}
        />
      </div>

      {/* Controls */}
      <div className="flex flex-wrap items-center gap-3">
        <TimeRangePicker value={timeRange} onChange={setTimeRange} />
        <button
          onClick={refreshRiskData}
          className="inline-flex items-center gap-2 px-4 py-2 text-sm font-medium text-white bg-shield hover:bg-shield/90 rounded-lg transition-colors"
        >
          <RefreshCw className="w-4 h-4" />
          Refresh
        </button>
      </div>

      {/* Summary metric strip */}
      {portfolioRisk ? (
        <div className="grid grid-cols-2 lg:grid-cols-4 gap-4">
          <SummaryTile label="Portfolio Heat">
            <div className="flex items-center gap-2">
              <span className={`text-2xl font-bold font-mono-numbers ${heatValueClass(heatStatus)}`}>
                {safeToFixed(portfolioRisk.portfolio_heat_percent, 1)}%
              </span>
              <Badge variant={heatBadgeVariant(heatStatus)}>{heatStatus}</Badge>
            </div>
          </SummaryTile>

          <SummaryTile label="Current Drawdown">
            <div className="text-2xl font-bold font-mono-numbers text-text">
              {safeToFixed(portfolioRisk.drawdown.current_drawdown_percent, 1)}%
            </div>
            <div className="text-xs text-text-muted mt-1 font-mono-numbers">
              Max {safeToFixed(portfolioRisk.drawdown.max_drawdown_percent, 1)}%
            </div>
          </SummaryTile>

          <SummaryTile label="Max Concentration">
            <div
              className={`text-2xl font-bold font-mono-numbers ${concentrationClass(
                portfolioRisk.concentration.max_concentration_percent
              )}`}
            >
              {safeToFixed(portfolioRisk.concentration.max_concentration_percent, 1)}%
            </div>
            <div className="text-xs text-text-muted mt-1">Single token</div>
          </SummaryTile>

          <SummaryTile label="Total Capital">
            <div className="text-2xl font-bold font-mono-numbers text-text">
              {safeToFixed(portfolioRisk.total_capital_sol, 4)} SOL
            </div>
            <div className="text-xs text-text-muted mt-1">Wallet balance</div>
          </SummaryTile>
        </div>
      ) : portfolioLoading ? (
        <Card>
          <CardContent className="py-10 text-center text-text-muted text-sm">
            Loading risk analysis…
          </CardContent>
        </Card>
      ) : (
        <Card className="border-loss/30">
          <CardContent className="py-10 text-center">
            <p className="text-loss font-medium mb-1">Failed to load risk data</p>
            <p className="text-text-muted text-sm">Please try again later</p>
          </CardContent>
        </Card>
      )}

      {/* Portfolio Heat & Concentration */}
      {portfolioRisk && (
        <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
          <PortfolioHeatChart
            heatPercentage={portfolioRisk.portfolio_heat_percent}
            heatThreshold={portfolioRisk.heat_threshold}
            heatStatus={portfolioRisk.heat_status === 'high' ? 'elevated' : portfolioRisk.heat_status}
          />

          <ConcentrationRiskChart
            byToken={portfolioRisk.concentration.by_token.map((token) => ({
              name: token.token_symbol || 'Unknown',
              value: token.total_value_sol,
              percentage: token.percentage,
            }))}
            bySector={portfolioRisk.concentration.by_sector.map((sector) => ({
              name: sector.sector,
              value: sector.total_value_sol,
              percentage: sector.percentage,
            }))}
            maxConcentrationPercent={portfolioRisk.concentration.max_concentration_percent}
            hhi={portfolioRisk.concentration.hhi}
          />
        </div>
      )}

      {/* Drawdown Analysis */}
      {portfolioRisk && (
        <DrawdownChart
          currentDrawdownPercent={portfolioRisk.drawdown.current_drawdown_percent}
          maxDrawdownPercent={portfolioRisk.drawdown.max_drawdown_percent}
          drawdownDurationDays={portfolioRisk.drawdown.drawdown_duration_days}
          recoveryPercent={portfolioRisk.drawdown.recovery_percent}
        />
      )}

      {/* Stop Loss & Profit Targets */}
      {stopLossMetrics && profitTargetMetrics ? (
        <StopLossProfitChart
          activationRate={stopLossMetrics.activation_rate}
          totalActivations={stopLossMetrics.total_activations}
          lossPreventedSol={stopLossMetrics.loss_prevented_sol}
          averageLossPreventedSol={stopLossMetrics.average_loss_prevented_sol}
          activationsByStrategy={stopLossMetrics.activations_by_strategy.map((sl) => ({
            strategy: sl.strategy,
            activations: sl.activations,
            lossPrevented: sl.loss_prevented_sol,
            averageLoss: sl.activations > 0 ? sl.loss_prevented_sol / sl.activations : 0,
          }))}
          hitRate={profitTargetMetrics.hit_rate}
          totalHits={profitTargetMetrics.total_hits}
          totalTargets={profitTargetMetrics.total_targets}
          trailingStopActivations={profitTargetMetrics.trailing_stop_activations}
          averageRealizedGainSol={profitTargetMetrics.average_realized_gain_sol}
          targetsByStrategy={profitTargetMetrics.targets_by_strategy.map((pt) => ({
            strategy: pt.strategy,
            hitRate: pt.hit_rate,
            totalHits: pt.total_hits,
            averageGain: pt.average_gain_sol,
          }))}
          recentHits={profitTargetMetrics.recent_hits.map((hit) => ({
            timestamp: hit.timestamp,
            token: hit.token_symbol || 'Unknown',
            gain: hit.realized_gain_sol,
          }))}
        />
      ) : (stopLossLoading || profitTargetLoading) && (
        <Card>
          <CardContent className="py-10 text-center text-text-muted text-sm">
            Loading stop-loss & profit-target metrics…
          </CardContent>
        </Card>
      )}

      {/* Position Sizing */}
      {positionSizeAnalysis ? (
        <>
          <Card>
            <CardHeader>
              <CardTitle>Position Sizing</CardTitle>
            </CardHeader>
            <CardContent className="space-y-6">
              <div className="grid grid-cols-2 lg:grid-cols-4 gap-4">
                <MetricCard
                  label="Avg Position"
                  value={`${safeToFixed(positionSizeAnalysis.average_position_sol, 2)} SOL`}
                />
                <MetricCard
                  label="Median Position"
                  value={`${safeToFixed(positionSizeAnalysis.median_position_sol, 2)} SOL`}
                />
                <MetricCard
                  label="Max Position"
                  value={`${safeToFixed(positionSizeAnalysis.max_position_sol, 2)} SOL`}
                />
                <MetricCard
                  label="Min Position"
                  value={`${safeToFixed(positionSizeAnalysis.min_position_sol, 2)} SOL`}
                />
              </div>

              <div>
                <h4 className="text-sm font-medium text-text-muted mb-3">Size Distribution</h4>
                <div className="space-y-3">
                  {positionSizeAnalysis.position_size_distribution.map((bucket) => (
                    <div key={bucket.range} className="flex items-center">
                      <div className="w-24 text-sm text-text-muted">{bucket.range}</div>
                      <div className="flex-1 mx-4">
                        <div className="w-full bg-surface-light rounded-full h-6">
                          <div
                            className="bg-shield h-6 rounded-full flex items-center justify-end pr-2 transition-all duration-300"
                            style={{ width: `${Math.max(bucket.percentage, 4)}%` }}
                          >
                            <span className="text-xs text-white font-medium font-mono-numbers">
                              {bucket.count}
                            </span>
                          </div>
                        </div>
                      </div>
                      <div className="w-16 text-right text-sm text-text-muted font-mono-numbers">
                        {safeToFixed(bucket.percentage, 0)}%
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            </CardContent>
          </Card>

          <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
            <MetricCard
              label="Kelly Criterion Usage"
              value={`${safeToFixed(positionSizeAnalysis.kelly_criterion_usage * 100, 0)}%`}
            />
            <MetricCard
              label="Size Categories"
              value={positionSizeAnalysis.position_size_distribution.length}
            />
            <MetricCard
              label="Total Positions"
              value={positionSizeAnalysis.position_size_distribution.reduce(
                (sum, bucket) => sum + bucket.count,
                0
              )}
            />
          </div>
        </>
      ) : positionSizeLoading ? (
        <Card>
          <CardContent className="py-10 text-center text-text-muted text-sm">
            Loading position-size analysis…
          </CardContent>
        </Card>
      ) : null}
    </div>
  )
}

function SummaryTile({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="bg-surface-light rounded-lg p-4">
      <div className="text-xs text-text-muted mb-2">{label}</div>
      {children}
    </div>
  )
}
