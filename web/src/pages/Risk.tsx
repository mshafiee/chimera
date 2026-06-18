import { Card, CardHeader, CardTitle, CardContent } from '../components/ui/Card'
import { Badge } from '../components/ui/Badge'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../components/ui/Table'
import { usePortfolioRisk, useStopLossMetrics, useProfitTargetMetrics } from '../api'
import { PortfolioHeatGauge } from '../components/risk/PortfolioHeatGauge'
import { ConcentrationMatrix } from '../components/risk/ConcentrationMatrix'
import { StopLossAnalytics } from '../components/risk/StopLossAnalytics'
import { ProfitTargetAnalytics } from '../components/risk/ProfitTargetAnalytics'
import { MetricCard } from '../components/ui/MetricCard'
import { AlertTriangle, Shield, Target } from 'lucide-react'
import { TimeRangePicker, TimeRange } from '../components/ui/TimeRangePicker'
import { useState } from 'react'

export function Risk() {
  const [timeRange, setTimeRange] = useState<TimeRange>('30d')

  const { data: portfolioRisk, isLoading: riskLoading } = usePortfolioRisk()
  const { data: stopLossMetrics, isLoading: slLoading } = useStopLossMetrics(timeRange)
  const { data: profitTargetMetrics, isLoading: ptLoading } = useProfitTargetMetrics(timeRange)

  const heatStatus = portfolioRisk?.heat_status || 'normal'
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

      {/* Portfolio Heat Alert */}
      {(heatStatus === 'high' || heatStatus === 'critical') && (
        <Card className={`border-2 ${heatStatus === 'critical' ? 'border-loss bg-loss/10' : 'border-spear bg-spear/10'}`}>
          <CardContent className="p-4">
            <div className="flex items-center gap-3">
              <AlertTriangle className={`w-6 h-6 ${heatColor}`} />
              <div className="flex-1">
                <div className={`font-semibold ${heatColor}`}>
                  {heatStatus === 'critical' ? 'Critical Risk Level' : 'Elevated Risk Level'}
                </div>
                <div className="text-sm text-text-muted">
                  Portfolio heat is at {portfolioRisk?.portfolio_heat_percent?.toFixed(1)}%.
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
          ) : portfolioRisk ? (
            <PortfolioHeatGauge data={portfolioRisk} />
          ) : (
            <div className="text-center text-text-muted py-8">No risk data available</div>
          )}
        </CardContent>
      </Card>

      {/* Concentration Analysis */}
      {portfolioRisk && (
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
      {portfolioRisk && (
        <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
          <MetricCard
            label="Current Drawdown"
            value={`${portfolioRisk.drawdown.current_drawdown_percent?.toFixed(1) || 0}%`}
            positive={(portfolioRisk.drawdown.current_drawdown_percent || 0) < 10}
            icon="📉"
          />
          <MetricCard
            label="Max Drawdown"
            value={`${portfolioRisk.drawdown.max_drawdown_percent?.toFixed(1) || 0}%`}
            icon="⚠️"
          />
          <MetricCard
            label="Recovery"
            value={`${portfolioRisk.drawdown.recovery_percent?.toFixed(1) || 0}%`}
            positive={(portfolioRisk.drawdown.recovery_percent || 0) > 0}
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
          ) : stopLossMetrics ? (
            <StopLossAnalytics data={stopLossMetrics} />
          ) : (
            <div className="text-center text-text-muted py-8">No stop loss data available</div>
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
          ) : profitTargetMetrics ? (
            <ProfitTargetAnalytics data={profitTargetMetrics} />
          ) : (
            <div className="text-center text-text-muted py-8">No profit target data available</div>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
