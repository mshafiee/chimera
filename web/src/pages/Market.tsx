import { Card, CardHeader, CardTitle, CardContent } from '../components/ui/Card'
import { Badge } from '../components/ui/Badge'
import { useMarketRegime, useMarketConditions } from '../api'
import { RegimeIndicator } from '../components/market/RegimeIndicator'
import { RegimeHistoryChart } from '../components/market/RegimeHistoryChart'
import { PerformanceByRegime } from '../components/market/PerformanceByRegime'
import { TrendingUp, TrendingDown, Minus, Activity } from 'lucide-react'

export function Market() {
  const { data: marketRegime, isLoading: regimeLoading } = useMarketRegime()
  const { data: marketConditions, isLoading: conditionsLoading } = useMarketConditions()

  return (
    <div className="space-y-6">
      {/* Header */}
      <div>
        <h1 className="text-2xl font-bold">Market Analysis</h1>
        <p className="text-text-muted text-sm">Regime detection and market conditions</p>
      </div>

      {/* Current Regime */}
      <Card>
        <CardHeader>
          <CardTitle>Current Market Regime</CardTitle>
        </CardHeader>
        <CardContent>
          {regimeLoading ? (
            <div className="text-center text-text-muted py-8">Loading market regime...</div>
          ) : marketRegime ? (
            <RegimeIndicator data={marketRegime} />
          ) : (
            <div className="text-center text-text-muted py-8">No regime data available</div>
          )}
        </CardContent>
      </Card>

      {/* Market Conditions */}
      <Card>
        <CardHeader>
          <CardTitle>Market Conditions</CardTitle>
        </CardHeader>
        <CardContent>
          {conditionsLoading ? (
            <div className="text-center text-text-muted py-8">Loading conditions...</div>
          ) : marketConditions ? (
            <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6">
              {/* Volatility */}
              <div className="bg-surface-light rounded-lg p-4">
                <div className="flex items-center gap-2 mb-2">
                  <Activity className="w-5 h-5 text-text-muted" />
                  <span className="text-sm text-text-muted">Volatility Index</span>
                </div>
                <div className="text-2xl font-bold font-mono-numbers">
                  {marketConditions.volatility_index.toFixed(2)}
                </div>
                <div className="text-xs text-text-muted mt-1">
                  {marketConditions.volatility_index < 20 ? 'Low' : marketConditions.volatility_index < 40 ? 'Moderate' : 'High'}
                </div>
              </div>

              {/* Trend Strength */}
              <div className="bg-surface-light rounded-lg p-4">
                <div className="flex items-center gap-2 mb-2">
                  {marketConditions.trend_strength > 0 ? (
                    <TrendingUp className="w-5 h-5 text-profit" />
                  ) : marketConditions.trend_strength < 0 ? (
                    <TrendingDown className="w-5 h-5 text-loss" />
                  ) : (
                    <Minus className="w-5 h-5 text-text-muted" />
                  )}
                  <span className="text-sm text-text-muted">Trend Strength</span>
                </div>
                <div className="text-2xl font-bold font-mono-numbers">
                  {Math.abs(marketConditions.trend_strength).toFixed(2)}
                </div>
                <div className="text-xs text-text-muted mt-1">
                  {marketConditions.trend_strength > 0.3 ? 'Strong Uptrend' : marketConditions.trend_strength < -0.3 ? 'Strong Downtrend' : 'Weak Trend'}
                </div>
              </div>

              {/* Liquidity */}
              <div className="bg-surface-light rounded-lg p-4">
                <div className="flex items-center gap-2 mb-2">
                  <Activity className="w-5 h-5 text-text-muted" />
                  <span className="text-sm text-text-muted">Liquidity Index</span>
                </div>
                <div className="text-2xl font-bold font-mono-numbers">
                  {marketConditions.liquidity_index.toFixed(2)}
                </div>
                <div className="text-xs text-text-muted mt-1">
                  {marketConditions.liquidity_index > 70 ? 'High' : marketConditions.liquidity_index > 40 ? 'Moderate' : 'Low'}
                </div>
              </div>

              {/* Market Sentiment */}
              <div className="bg-surface-light rounded-lg p-4">
                <div className="text-sm text-text-muted mb-2">Market Sentiment</div>
                <Badge
                  variant={marketConditions.market_sentiment === 'bullish' ? 'success' : marketConditions.market_sentiment === 'bearish' ? 'danger' : 'default'}
                  size="md"
                >
                  {marketConditions.market_sentiment}
                </Badge>
              </div>

              {/* Risk Level */}
              <div className="bg-surface-light rounded-lg p-4">
                <div className="text-sm text-text-muted mb-2">Risk Level</div>
                <Badge
                  variant={marketConditions.risk_level === 'low' ? 'success' : marketConditions.risk_level === 'medium' ? 'warning' : 'danger'}
                  size="md"
                >
                  {marketConditions.risk_level}
                </Badge>
              </div>

              {/* Recommended Allocation */}
              <div className="bg-surface-light rounded-lg p-4">
                <div className="text-sm text-text-muted mb-2">Recommended Allocation</div>
                <div className="flex items-center gap-4">
                  <div>
                    <div className="text-xs text-text-muted">Shield</div>
                    <div className="text-lg font-bold text-shield">
                      {marketConditions.recommended_allocation.shield_percent}%
                    </div>
                  </div>
                  <div>
                    <div className="text-xs text-text-muted">Spear</div>
                    <div className="text-lg font-bold text-spear">
                      {marketConditions.recommended_allocation.spear_percent}%
                    </div>
                  </div>
                </div>
              </div>
            </div>
          ) : (
            <div className="text-center text-text-muted py-8">No conditions data available</div>
          )}
        </CardContent>
      </Card>

      {/* Regime History */}
      {marketRegime && marketRegime.regime_history.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle>Regime History</CardTitle>
          </CardHeader>
          <CardContent>
            <RegimeHistoryChart history={marketRegime.regime_history} />
          </CardContent>
        </Card>
      )}

      {/* Performance by Regime */}
      {marketRegime && marketRegime.performance_by_regime.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle>Performance by Regime</CardTitle>
          </CardHeader>
          <CardContent>
            <PerformanceByRegime data={marketRegime.performance_by_regime} />
          </CardContent>
        </Card>
      )}
    </div>
  )
}
