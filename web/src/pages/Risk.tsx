import { Card, CardHeader, CardTitle, CardContent } from '../components/ui/Card'
import { usePortfolioRisk, useStopLossMetrics, useProfitTargetMetrics } from '../api'
import { PortfolioHeatGauge } from '../components/risk/PortfolioHeatGauge'
import { ConcentrationMatrix } from '../components/risk/ConcentrationMatrix'
import { StopLossAnalytics } from '../components/risk/StopLossAnalytics'
import { ProfitTargetAnalytics } from '../components/risk/ProfitTargetAnalytics'
import { MetricCard } from '../components/ui/MetricCard'
import { AlertTriangle, Shield, Target, Info } from 'lucide-react'
import { TimeRangePicker, TimeRange } from '../components/ui/TimeRangePicker'
import { useState } from 'react'

// Helper function to safely access nested properties
const safeGet = <T,>(obj: any, path: string, defaultValue: T): T => {
  try {
    const keys = path.split('.')
    let result = obj
    for (const key of keys) {
      if (result && typeof result === 'object' && key in result) {
        result = result[key]
      } else {
        return defaultValue
      }
    }
    return result as T
  } catch {
    return defaultValue
  }
}

export function Risk() {
  const [timeRange, setTimeRange] = useState<TimeRange>('30d')

  const { data: portfolioRisk, isLoading: riskLoading, error: riskError } = usePortfolioRisk()
  const { data: stopLossMetrics, isLoading: slLoading } = useStopLossMetrics(timeRange)
  const { data: profitTargetMetrics, isLoading: ptLoading } = useProfitTargetMetrics(timeRange)

  // Check if we have proper portfolio risk data structure
  const hasPortfolioRiskData = portfolioRisk && 'portfolio_heat_percent' in portfolioRisk
  const heatStatus = safeGet(portfolioRisk, 'heat_status', 'normal') as 'normal' | 'elevated' | 'high' | 'critical'
  const heatColor = {
    normal: 'text-profit',
    elevated: 'text-spear',
    high: 'text-loss',
    critical: 'text-loss',
  }[heatStatus]

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold">Risk Management</h1>
          <p className="text-text-muted text-sm">Portfolio risk and protection metrics</p>
        </div>
        <TimeRangePicker value={timeRange} onChange={setTimeRange} />
      </div>

      {/* API Not Available Notice */}
      {!hasPortfolioRiskData && !riskLoading && (
        <Card className="border-spear bg-spear/10">
          <CardContent className="p-4">
            <div className="flex items-center gap-3">
              <Info className="w-5 h-5 text-spear" />
              <div className="flex-1">
                <div className="font-semibold text-spear">
                  Risk API Endpoint Not Implemented
                </div>
                <div className="text-sm text-text-muted">
                  The comprehensive portfolio risk API endpoint is not yet available.
                  Risk metrics will be displayed once the backend implementation is complete.
                </div>
              </div>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Portfolio Heat Alert */}
      {hasPortfolioRiskData && (heatStatus === 'high' || heatStatus === 'critical') && (
        <Card className={`border-2 ${heatStatus === 'critical' ? 'border-loss bg-loss/10' : 'border-spear bg-spear/10'}`}>
          <CardContent className="p-4">
            <div className="flex items-center gap-3">
              <AlertTriangle className={`w-6 h-6 ${heatColor}`} />
              <div className="flex-1">
                <div className={`font-semibold ${heatColor}`}>
                  {heatStatus === 'critical' ? 'Critical Risk Level' : 'Elevated Risk Level'}
                </div>
                <div className="text-sm text-text-muted">
                  Portfolio heat is at {safeGet(portfolioRisk, 'portfolio_heat_percent', 0)?.toFixed(1)}%.
                  Consider reducing position sizes.
                </div>
              </div>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Portfolio Heat */}
      <Card>
        <CardHeader>
          <CardTitle>Portfolio Heat</CardTitle>
        </CardHeader>
        <CardContent>
          {riskLoading ? (
            <div className="text-center text-text-muted py-8">Loading risk data...</div>
          ) : riskError ? (
            <div className="text-center text-loss py-8">Error loading risk data</div>
          ) : hasPortfolioRiskData ? (
            <PortfolioHeatGauge data={portfolioRisk} />
          ) : (
            <div className="text-center text-text-muted py-8">
              Risk metrics not available - API endpoint not implemented
            </div>
          )}
        </CardContent>
      </Card>

      {/* Concentration Analysis */}
      {hasPortfolioRiskData && portfolioRisk?.concentration && (
        <Card>
          <CardHeader>
            <CardTitle>Concentration Analysis</CardTitle>
          </CardHeader>
          <CardContent>
            <ConcentrationMatrix data={portfolioRisk.concentration} />
          </CardContent>
        </Card>
      )}

      {/* Drawdown Status */}
      {hasPortfolioRiskData && portfolioRisk?.drawdown && (
        <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
          <MetricCard
            label="Current Drawdown"
            value={`${safeGet(portfolioRisk, 'drawdown.current_drawdown_percent', 0)?.toFixed(1) || 0}%`}
            positive={safeGet(portfolioRisk, 'drawdown.current_drawdown_percent', 0) < 10}
            icon="📉"
          />
          <MetricCard
            label="Max Drawdown"
            value={`${safeGet(portfolioRisk, 'drawdown.max_drawdown_percent', 0)?.toFixed(1) || 0}%`}
            icon="⚠️"
          />
          <MetricCard
            label="Recovery"
            value={`${safeGet(portfolioRisk, 'drawdown.recovery_percent', 0)?.toFixed(1) || 0}%`}
            positive={safeGet(portfolioRisk, 'drawdown.recovery_percent', 0) > 0}
            icon="🔄"
          />
        </div>
      )}

      {/* Stop Loss Analytics */}
      <Card>
        <CardHeader>
          <div className="flex items-center gap-2">
            <Shield className="w-5 h-5 text-loss" />
            <CardTitle>Stop Loss Metrics</CardTitle>
          </div>
        </CardHeader>
        <CardContent>
          {slLoading ? (
            <div className="text-center text-text-muted py-8">Loading stop loss data...</div>
          ) : stopLossMetrics && 'activation_rate' in stopLossMetrics ? (
            <StopLossAnalytics data={stopLossMetrics} />
          ) : (
            <div className="text-center text-text-muted py-8">
              Stop loss metrics not available - API endpoint not implemented
            </div>
          )}
        </CardContent>
      </Card>

      {/* Profit Target Analytics */}
      <Card>
        <CardHeader>
          <div className="flex items-center gap-2">
            <Target className="w-5 h-5 text-profit" />
            <CardTitle>Profit Target Metrics</CardTitle>
          </div>
        </CardHeader>
        <CardContent>
          {ptLoading ? (
            <div className="text-center text-text-muted py-8">Loading profit target data...</div>
          ) : profitTargetMetrics && 'hit_rate' in profitTargetMetrics ? (
            <ProfitTargetAnalytics data={profitTargetMetrics} />
          ) : (
            <div className="text-center text-text-muted py-8">
              Profit target metrics not available - API endpoint not implemented
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
