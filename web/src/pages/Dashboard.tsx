import { useEffect, useMemo } from 'react'
import { ExternalLink, AlertTriangle } from 'lucide-react'
import { Card, CardHeader, CardTitle, CardContent } from '../components/ui/Card'
import { Badge, StrategyBadge, StatusBadge } from '../components/ui/Badge'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../components/ui/Table'
import { PnLChart } from '../components/charts/PnLChart'
import { useHealth, usePositions } from '../api'
import { useCostMetrics, usePerformanceMetrics, useStrategyPerformance } from '../api/metrics'
import { useTrades } from '../api/trades'
import { useConfig } from '../api/config'
import { useLayoutContext } from '../components/layout/Layout'
import { useWebSocket } from '../hooks/useWebSocket'
import { toast } from '../components/ui/Toast'

export function Dashboard() {
  const { setLastUpdate } = useLayoutContext()
  const { data: health, refetch: refetchHealth } = useHealth()
  const { data: positionsData, isLoading: positionsLoading, refetch: refetchPositions } = usePositions()
  const { data: performanceMetrics, isLoading: metricsLoading } = usePerformanceMetrics()
  const { data: costMetrics, isLoading: costMetricsLoading } = useCostMetrics()
  const { data: shieldPerformance } = useStrategyPerformance('SHIELD', 30)
  const { data: spearPerformance } = useStrategyPerformance('SPEAR', 30)
  const { data: configData } = useConfig()
  
  // Fetch trades for PnL chart (last 30 days)
  const thirtyDaysAgo = useMemo(() => {
    const d = new Date()
    d.setDate(d.getDate() - 30)
    return d.toISOString()
  }, [])
  const { data: tradesData } = useTrades({ from: thirtyDaysAgo, status: 'CLOSED', limit: 1000 })
  
  // WebSocket for real-time updates
  const { isConnected, lastMessage } = useWebSocket()

  // Update last update time when data changes
  useEffect(() => {
    if (health || positionsData) {
      setLastUpdate(new Date())
    }
  }, [health, positionsData, setLastUpdate])

  // Handle WebSocket messages
  useEffect(() => {
    if (!lastMessage) return

    switch (lastMessage.type) {
      case 'position_update':
      case 'trade_update':
        refetchPositions()
        break
      case 'health_update':
        refetchHealth()
        break
      case 'alert':
        // Show toast notification for alerts
        const alertData = lastMessage.data as { severity?: string; component?: string; message?: string }
        const severity = alertData?.severity || 'info'
        const message = alertData?.message || 'Alert received'
        if (severity === 'critical') {
          toast.error(message, 10000) // Show critical alerts longer
        } else if (severity === 'warning') {
          toast.warning(message)
        } else {
          toast.info(message)
        }
        break
    }
  }, [lastMessage, refetchPositions, refetchHealth])

  const positions = positionsData?.positions || []
  const activePositions = positions.filter((p) => p.state === 'ACTIVE')

  // Compute PnL data from actual trades
  const pnlData = useMemo(() => {
    if (!tradesData?.trades || tradesData.trades.length === 0) {
      // Return empty data - no sample data
      return []
    }
    
    // Group trades by date and compute cumulative PnL
    const pnlByDate = new Map<string, number>()
    let cumPnl = 0
    
    // Sort trades by date
    const sortedTrades = [...tradesData.trades].sort((a, b) => 
      new Date(a.created_at).getTime() - new Date(b.created_at).getTime()
    )
    
    for (const trade of sortedTrades) {
      const dateStr = new Date(trade.created_at).toLocaleDateString('en-US', { month: 'short', day: 'numeric' })
      cumPnl += trade.pnl_usd || 0
      pnlByDate.set(dateStr, cumPnl)
    }
    
    return Array.from(pnlByDate.entries()).map(([date, pnl]) => ({
      date,
      pnl: Math.round(pnl * 100) / 100
    }))
  }, [tradesData])

  return (
    <div className="space-y-6">
      {/* System Halted Banner - Prominent Alert */}
      {health && !health.circuit_breaker.trading_allowed && (
        <Card className="bg-loss/10 border-loss border-2">
          <CardContent className="p-4">
            <div className="flex items-center gap-3">
              <AlertTriangle className="w-6 h-6 text-loss flex-shrink-0" />
              <div className="flex-1">
                <div className="font-semibold text-loss mb-1">Trading Halted</div>
                <div className="text-sm text-text-muted">
                  {health.circuit_breaker.trip_reason || 'Trading has been halted by the kill switch or circuit breaker.'}
                  {health.circuit_breaker.cooldown_remaining_secs && (
                    <span className="ml-2">
                      Cooldown: {Math.floor(health.circuit_breaker.cooldown_remaining_secs / 60)}m {health.circuit_breaker.cooldown_remaining_secs % 60}s
                    </span>
                  )}
                </div>
              </div>
              <Badge variant="danger" size="sm">
                {health.circuit_breaker.state}
              </Badge>
            </div>
          </CardContent>
        </Card>
      )}
      
      {/* Simplified Mobile View - Key Metrics Only */}
      <div className="md:hidden space-y-3">
        <Card padding="sm">
          <div className="space-y-3">
            {/* Critical Status */}
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2">
                <span
                  className={`status-dot ${
                    health?.status === 'healthy'
                      ? 'status-dot-healthy'
                      : health?.status === 'degraded'
                      ? 'status-dot-degraded'
                      : 'status-dot-unhealthy'
                  }`}
                />
                <span className="text-xs font-medium uppercase">
                  {health?.status || 'Unknown'}
                </span>
              </div>
              <div className="flex items-center gap-3 text-xs">
                <span className="font-mono-numbers">◎ {activePositions.length} Pos</span>
                <span className="font-mono-numbers">Q:{health?.queue_depth || 0}</span>
              </div>
            </div>
            
            {/* Key PnL - 24h only on mobile */}
            <div className="border-t border-border pt-3">
              <div className="text-xs text-text-muted mb-1">24h PnL</div>
              <div className={`text-2xl font-bold font-mono-numbers ${
                performanceMetrics && Number(performanceMetrics.pnl_24h) >= 0
                  ? 'text-profit'
                  : 'text-loss'
              }`}>
                {metricsLoading
                  ? '...'
                  : performanceMetrics && performanceMetrics.pnl_24h != null
                  ? `${Number(performanceMetrics.pnl_24h) >= 0 ? '+' : ''}$${safeToFixed(performanceMetrics.pnl_24h, 2)}`
                  : '$0.00'}
              </div>
              {performanceMetrics?.pnl_24h_change_percent != null && (
                <div className={`text-xs mt-1 ${
                  Number(performanceMetrics.pnl_24h_change_percent) >= 0
                    ? 'text-profit'
                    : 'text-loss'
                }`}>
                  {Number(performanceMetrics.pnl_24h_change_percent) >= 0 ? '+' : ''}
                  {safeToFixed(performanceMetrics.pnl_24h_change_percent, 1)}%
                </div>
              )}
            </div>
            
            {/* Circuit Breaker Status - Critical on Mobile */}
            <div className="flex items-center justify-between border-t border-border pt-3">
              <span className="text-xs text-text-muted">Circuit Breaker</span>
              <Badge
                variant={health?.circuit_breaker?.trading_allowed ? 'success' : 'danger'}
                size="sm"
              >
                {health?.circuit_breaker?.trading_allowed ? 'Active' : 'Tripped'}
              </Badge>
            </div>
          </div>
        </Card>
        
        {/* Quick Actions - Emergency Halt */}
        {!health?.circuit_breaker?.trading_allowed && (
          <Card padding="sm" className="bg-danger/10 border-danger/20">
            <div className="text-xs text-center text-text-muted">
              Trading is currently halted
            </div>
          </Card>
        )}
      </div>
      
      {/* System Status Bar - Desktop/Tablet */}
      <Card padding="sm" className="hidden md:block">
        <div className="flex flex-col gap-3 md:gap-4 md:flex-row md:items-center md:justify-between">
          <div className="flex flex-wrap items-center gap-3 md:gap-6">
            {/* System Health */}
            <div className="flex items-center gap-2">
              <span
                className={`status-dot ${
                  health?.status === 'healthy'
                    ? 'status-dot-healthy'
                    : health?.status === 'degraded'
                    ? 'status-dot-degraded'
                    : 'status-dot-unhealthy'
                }`}
              />
              <span className="text-xs md:text-sm font-medium uppercase">
                <span className="hidden sm:inline">System </span>
                {health?.status || 'Unknown'}
              </span>
              {/* WebSocket connection indicator */}
              <span
                className={`w-2 h-2 rounded-full ${
                  isConnected ? 'bg-green-500' : 'bg-red-500'
                }`}
                title={isConnected ? 'WebSocket connected' : 'WebSocket disconnected'}
              />
            </div>

            {/* Balance - shows when wallet is connected */}
            <div className="hidden xs:flex items-center gap-2 text-xs md:text-sm">
              <span className="text-text-muted">Balance:</span>
              <span className="font-mono-numbers font-semibold text-text-muted">—</span>
            </div>

            {/* NAV - computed from positions */}
            <div className="hidden sm:flex items-center gap-2 text-xs md:text-sm">
              <span className="text-text-muted">NAV:</span>
              <span className="font-mono-numbers font-semibold text-text-muted">—</span>
            </div>
          </div>

          <div className="flex flex-wrap items-center gap-2 md:gap-4 text-xs md:text-sm">
            {/* Circuit Breaker Status */}
            <div className="flex items-center gap-1.5 md:gap-2">
              <span className="text-text-muted hidden sm:inline">Circuit Breaker:</span>
              <span className="text-text-muted sm:hidden">CB:</span>
              <Badge
                variant={health?.circuit_breaker?.trading_allowed ? 'success' : 'danger'}
                size="sm"
              >
                {health?.circuit_breaker?.trading_allowed ? 'Active' : 'Tripped'}
              </Badge>
            </div>

            {/* Queue Depth */}
            <div className="flex items-center gap-1.5 md:gap-2">
              <span className="text-text-muted">Queue:</span>
              <span className="font-mono-numbers">{health?.queue_depth || 0}/1000</span>
            </div>

            {/* Uptime - hidden on very small screens */}
            <div className="hidden xs:flex items-center gap-1.5 md:gap-2">
              <span className="text-text-muted">Uptime:</span>
              <span className="font-mono-numbers">
                {formatUptime(health?.uptime_seconds || 0)}
              </span>
            </div>
          </div>
        </div>
      </Card>

      {/* Performance Metrics */}
      <Card>
        <CardHeader>
          <CardTitle>Performance</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="grid grid-cols-1 sm:grid-cols-3 gap-4 mb-4">
            <MetricCard
              label="24h"
              value={
                metricsLoading
                  ? '...'
                  : performanceMetrics && performanceMetrics.pnl_24h != null
                  ? `${Number(performanceMetrics.pnl_24h) >= 0 ? '+' : ''}$${safeToFixed(performanceMetrics.pnl_24h, 2)}`
                  : '$0.00'
              }
              change={
                performanceMetrics?.pnl_24h_change_percent !== undefined && performanceMetrics.pnl_24h_change_percent !== null
                  ? `${Number(performanceMetrics.pnl_24h_change_percent) >= 0 ? '+' : ''}${safeToFixed(performanceMetrics.pnl_24h_change_percent, 1)}%`
                  : undefined
              }
              positive={performanceMetrics ? Number(performanceMetrics.pnl_24h) >= 0 : true}
            />
            <MetricCard
              label="7d"
              value={
                metricsLoading
                  ? '...'
                  : performanceMetrics && performanceMetrics.pnl_7d != null
                  ? `${Number(performanceMetrics.pnl_7d) >= 0 ? '+' : ''}$${safeToFixed(performanceMetrics.pnl_7d, 2)}`
                  : '$0.00'
              }
              change={
                performanceMetrics?.pnl_7d_change_percent !== undefined && performanceMetrics.pnl_7d_change_percent !== null
                  ? `${Number(performanceMetrics.pnl_7d_change_percent) >= 0 ? '+' : ''}${safeToFixed(performanceMetrics.pnl_7d_change_percent, 1)}%`
                  : undefined
              }
              positive={performanceMetrics ? Number(performanceMetrics.pnl_7d) >= 0 : true}
            />
            <MetricCard
              label="30d"
              value={
                metricsLoading
                  ? '...'
                  : performanceMetrics && performanceMetrics.pnl_30d != null
                  ? `${Number(performanceMetrics.pnl_30d) >= 0 ? '+' : ''}$${safeToFixed(performanceMetrics.pnl_30d, 2)}`
                  : '$0.00'
              }
              change={
                performanceMetrics?.pnl_30d_change_percent !== undefined && performanceMetrics.pnl_30d_change_percent !== null
                  ? `${Number(performanceMetrics.pnl_30d_change_percent) >= 0 ? '+' : ''}${safeToFixed(performanceMetrics.pnl_30d_change_percent, 1)}%`
                  : undefined
              }
              positive={performanceMetrics ? Number(performanceMetrics.pnl_30d) >= 0 : true}
            />
          </div>
          {pnlData.length > 0 ? (
            <PnLChart data={pnlData} />
          ) : (
            <div className="h-[200px] flex items-center justify-center text-text-muted text-sm">
              No trade history available
            </div>
          )}
        </CardContent>
      </Card>

      {/* Cost Breakdown */}
      <Card>
        <CardHeader>
          <CardTitle>Cost Analysis (30d)</CardTitle>
        </CardHeader>
        <CardContent>
          {costMetricsLoading ? (
            <div className="text-center text-text-muted py-8">Loading cost metrics...</div>
          ) : costMetrics ? (
            <div className="space-y-4">
              <div className="grid grid-cols-2 md:grid-cols-3 gap-4">
                <div>
                  <div className="text-xs md:text-sm text-text-muted mb-1">Avg Jito Tip</div>
                  <div className="text-lg md:text-xl font-semibold font-mono-numbers">
                    {safeToFixed(costMetrics.avg_jito_tip_sol, 4)} SOL
                  </div>
                </div>
                <div>
                  <div className="text-xs md:text-sm text-text-muted mb-1">Avg DEX Fee</div>
                  <div className="text-lg md:text-xl font-semibold font-mono-numbers">
                    {safeToFixed(costMetrics.avg_dex_fee_sol, 4)} SOL
                  </div>
                </div>
                <div>
                  <div className="text-xs md:text-sm text-text-muted mb-1">Avg Slippage</div>
                  <div className="text-lg md:text-xl font-semibold font-mono-numbers">
                    {safeToFixed(costMetrics.avg_slippage_cost_sol, 4)} SOL
                  </div>
                </div>
              </div>
              <div className="border-t border-border pt-4">
                <div className="grid grid-cols-2 md:grid-cols-3 gap-4">
                  <div>
                    <div className="text-xs md:text-sm text-text-muted mb-1">Total Costs (30d)</div>
                    <div className="text-lg md:text-xl font-semibold font-mono-numbers text-text-muted">
                      {safeToFixed(costMetrics.total_costs_30d_sol, 4)} SOL
                    </div>
                  </div>
                  <div>
                    <div className="text-xs md:text-sm text-text-muted mb-1">Net Profit (30d)</div>
                    <div className={`text-lg md:text-xl font-semibold font-mono-numbers ${
                      Number(costMetrics.net_profit_30d_sol) >= 0 ? 'text-profit' : 'text-loss'
                    }`}>
                      {Number(costMetrics.net_profit_30d_sol) >= 0 ? '+' : ''}
                      {safeToFixed(costMetrics.net_profit_30d_sol, 4)} SOL
                    </div>
                  </div>
                  <div>
                    <div className="text-xs md:text-sm text-text-muted mb-1">ROI %</div>
                    <div className={`text-lg md:text-xl font-semibold font-mono-numbers ${
                      Number(costMetrics.roi_percent) >= 0 ? 'text-profit' : 'text-loss'
                    }`}>
                      {Number(costMetrics.roi_percent) >= 0 ? '+' : ''}
                      {safeToFixed(costMetrics.roi_percent, 1)}%
                    </div>
                  </div>
                </div>
              </div>
            </div>
          ) : (
            <div className="text-center text-text-muted py-8">No cost data available</div>
          )}
        </CardContent>
      </Card>

      {/* Strategy Breakdown */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-4 md:gap-6">
        <Card variant="shield">
          <CardHeader>
            <CardTitle>🛡️ Shield Strategy</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="grid grid-cols-3 gap-2 md:gap-4 mb-4">
              <div>
                <div className="text-xs md:text-sm text-text-muted">Win Rate</div>
                <div className="text-xl md:text-2xl font-semibold font-mono-numbers text-shield">
                  {shieldPerformance
                    ? `${safeToFixed(shieldPerformance.win_rate, 1)}%`
                    : '...'}
                </div>
              </div>
              <div>
                <div className="text-xs md:text-sm text-text-muted">Avg Return</div>
                <div className={`text-xl md:text-2xl font-semibold font-mono-numbers ${
                  shieldPerformance && Number(shieldPerformance.avg_return) >= 0
                    ? 'text-profit'
                    : 'text-loss'
                }`}>
                  {shieldPerformance
                    ? `${Number(shieldPerformance.avg_return) >= 0 ? '+' : ''}$${safeToFixed(shieldPerformance.avg_return, 2)}`
                    : '...'}
                </div>
              </div>
              <div>
                <div className="text-xs md:text-sm text-text-muted">Positions</div>
                <div className="text-xl md:text-2xl font-semibold font-mono-numbers">
                  {activePositions.filter((p) => p.strategy === 'SHIELD').length}
                </div>
              </div>
            </div>
            <div className="mt-4">
              <div className="flex justify-between text-sm mb-1">
                <span className="text-text-muted">Allocation</span>
                <span className="font-mono-numbers">
                  {configData?.strategy_allocation?.shield_percent ?? '—'}%
                </span>
              </div>
              <div className="h-2 bg-background rounded-full overflow-hidden">
                <div 
                  className="h-full bg-shield transition-all duration-300" 
                  style={{ width: `${configData?.strategy_allocation?.shield_percent ?? 0}%` }}
                />
              </div>
            </div>
          </CardContent>
        </Card>

        <Card variant="spear">
          <CardHeader>
            <CardTitle>⚔️ Spear Strategy</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="grid grid-cols-3 gap-2 md:gap-4 mb-4">
              <div>
                <div className="text-xs md:text-sm text-text-muted">Win Rate</div>
                <div className="text-xl md:text-2xl font-semibold font-mono-numbers text-spear">
                  {spearPerformance
                    ? `${safeToFixed(spearPerformance.win_rate, 1)}%`
                    : '...'}
                </div>
              </div>
              <div>
                <div className="text-xs md:text-sm text-text-muted">Avg Return</div>
                <div className={`text-xl md:text-2xl font-semibold font-mono-numbers ${
                  spearPerformance && Number(spearPerformance.avg_return) >= 0
                    ? 'text-profit'
                    : 'text-loss'
                }`}>
                  {spearPerformance
                    ? `${Number(spearPerformance.avg_return) >= 0 ? '+' : ''}$${safeToFixed(spearPerformance.avg_return, 2)}`
                    : '...'}
                </div>
              </div>
              <div>
                <div className="text-xs md:text-sm text-text-muted">Positions</div>
                <div className="text-xl md:text-2xl font-semibold font-mono-numbers">
                  {activePositions.filter((p) => p.strategy === 'SPEAR').length}
                </div>
              </div>
            </div>
            <div className="mt-4">
              <div className="flex justify-between text-sm mb-1">
                <span className="text-text-muted">Allocation</span>
                <span className="font-mono-numbers">
                  {configData?.strategy_allocation?.spear_percent ?? '—'}%
                </span>
              </div>
              <div className="h-2 bg-background rounded-full overflow-hidden">
                <div 
                  className="h-full bg-spear transition-all duration-300" 
                  style={{ width: `${configData?.strategy_allocation?.spear_percent ?? 0}%` }}
                />
              </div>
            </div>
          </CardContent>
        </Card>
      </div>

      {/* System Health */}
      <Card>
        <CardHeader>
          <CardTitle>System Health</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
            <HealthIndicator
              name="Database"
              status={health?.database.status || 'unknown'}
            />
            <HealthIndicator
              name="RPC"
              status={health?.rpc.status || 'unknown'}
            />
            <HealthIndicator
              name="Helius"
              status={
                configData?.rpc_status?.primary === 'helius' 
                  ? (configData?.rpc_status?.fallback_triggered ? 'degraded' : 'healthy')
                  : 'unknown'
              }
            />
            <HealthIndicator
              name="Jito"
              status={
                !configData?.jito_enabled
                  ? 'unknown' // Disabled - show as unknown/white
                  : configData?.rpc_status?.active === 'jito'
                  ? 'healthy'
                  : (configData?.rpc_status?.fallback_triggered ? 'degraded' : 'unknown')
              }
            />
          </div>
        </CardContent>
      </Card>

      {/* Live Positions - Mobile Optimized */}
      <Card padding="none">
        <div className="p-3 md:p-4 border-b border-border">
          <CardTitle className="text-base md:text-lg">Live Positions</CardTitle>
        </div>
        {positionsLoading ? (
          <div className="p-6 md:p-8 text-center text-text-muted text-sm">Loading positions...</div>
        ) : positions.length === 0 ? (
          <div className="p-6 md:p-8 text-center text-text-muted text-sm">No active positions</div>
        ) : (
          <div className="overflow-x-auto -mx-4 md:mx-0">
          <div className="inline-block min-w-full align-middle px-4 md:px-0">
          <Table>
            <TableHeader>
              <TableRow hoverable={false}>
                <TableHead className="text-xs md:text-sm">Token</TableHead>
                <TableHead className="hidden sm:table-cell text-xs md:text-sm">Strategy</TableHead>
                <TableHead className="text-xs md:text-sm">Size</TableHead>
                <TableHead className="hidden md:table-cell text-xs md:text-sm">Entry</TableHead>
                <TableHead className="hidden lg:table-cell text-xs md:text-sm">Current</TableHead>
                <TableHead className="text-xs md:text-sm">PnL</TableHead>
                <TableHead className="hidden sm:table-cell text-xs md:text-sm">Status</TableHead>
                <TableHead className="text-xs md:text-sm"></TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {positions.slice(0, 10).map((position) => (
                <TableRow key={position.trade_uuid}>
                  <TableCell className="text-xs md:text-sm">
                    <div className="font-semibold">
                      ${position.token_symbol || 'Unknown'}
                    </div>
                    <div className="text-xs text-text-muted">
                      {position.token_address.slice(0, 8)}...
                    </div>
                    {/* Show strategy on mobile in token cell */}
                    <div className="sm:hidden mt-1">
                      <StrategyBadge strategy={position.strategy} />
                    </div>
                  </TableCell>
                  <TableCell className="hidden sm:table-cell">
                    <StrategyBadge strategy={position.strategy} />
                  </TableCell>
                  <TableCell mono className="text-xs md:text-sm">
                    {position.entry_amount_sol.toFixed(4)} SOL
                  </TableCell>
                  <TableCell mono className="hidden md:table-cell text-xs md:text-sm">
                    {position.entry_price.toFixed(8)}
                  </TableCell>
                  <TableCell mono className="hidden lg:table-cell text-xs md:text-sm">
                    {position.current_price?.toFixed(8) || '-'}
                  </TableCell>
                  <TableCell mono className="text-xs md:text-sm">
                    {position.unrealized_pnl_percent !== null ? (
                      <span
                        className={
                          position.unrealized_pnl_percent >= 0
                            ? 'text-profit'
                            : 'text-loss'
                        }
                      >
                        {position.unrealized_pnl_percent >= 0 ? '+' : ''}
                        {position.unrealized_pnl_percent.toFixed(2)}%
                      </span>
                    ) : (
                      '-'
                    )}
                  </TableCell>
                  <TableCell className="hidden sm:table-cell">
                    <StatusBadge status={position.state} />
                  </TableCell>
                  <TableCell className="text-xs md:text-sm">
                    {position.entry_tx_signature && (
                      <a
                        href={`https://solscan.io/tx/${position.entry_tx_signature}`}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="text-shield hover:text-shield-dark"
                      >
                        <ExternalLink className="w-3.5 h-3.5 md:w-4 md:h-4" />
                      </a>
                    )}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
          </div>
          </div>
        )}
      </Card>
    </div>
  )
}

// Helper components
function MetricCard({
  label,
  value,
  change,
  positive,
}: {
  label: string
  value: string
  change?: string
  positive: boolean
}) {
  return (
    <div className="bg-surface-light rounded-lg p-4">
      <div className="text-sm text-text-muted mb-1">{label}</div>
      <div className="text-2xl font-semibold font-mono-numbers">{value}</div>
      {change && (
        <div
          className={`text-sm font-mono-numbers ${
            positive ? 'text-profit' : 'text-loss'
          }`}
        >
          {change}
        </div>
      )}
    </div>
  )
}

function HealthIndicator({
  name,
  status,
}: {
  name: string
  status: string
}) {
  const statusColors = {
    healthy: 'text-profit',
    degraded: 'text-spear',
    unhealthy: 'text-loss',
    unknown: 'text-text-muted',
  }

  const statusIcons = {
    healthy: '🟢',
    degraded: '🟡',
    unhealthy: '🔴',
    unknown: '⚪',
  }

  const color = statusColors[status as keyof typeof statusColors] || statusColors.unknown
  const icon = statusIcons[status as keyof typeof statusIcons] || statusIcons.unknown

  return (
    <div className="flex items-center gap-2">
      <span>{icon}</span>
      <span className={`text-sm ${color}`}>{name}</span>
    </div>
  )
}

function formatUptime(seconds: number): string {
  const days = Math.floor(seconds / 86400)
  const hours = Math.floor((seconds % 86400) / 3600)
  const minutes = Math.floor((seconds % 3600) / 60)

  if (days > 0) {
    return `${days}d ${hours}h ${minutes}m`
  }
  if (hours > 0) {
    return `${hours}h ${minutes}m`
  }
  return `${minutes}m`
}

// Helper function to safely convert and format numbers
function safeToFixed(value: unknown, decimals: number = 2): string {
  if (value === null || value === undefined) {
    return '0.' + '0'.repeat(decimals)
  }
  const num = typeof value === 'number' ? value : Number(value)
  if (isNaN(num)) {
    return '0.' + '0'.repeat(decimals)
  }
  return num.toFixed(decimals)
}
