import { useEffect } from 'react'
import { ExternalLink } from 'lucide-react'
import { Card, CardHeader, CardTitle, CardContent } from '../components/ui/Card'
import { Badge, StrategyBadge, StatusBadge } from '../components/ui/Badge'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../components/ui/Table'
import { PnLChart, generateSamplePnLData } from '../components/charts/PnLChart'
import { useHealth, usePositions } from '../api'
import { useLayoutContext } from '../components/layout/Layout'

export function Dashboard() {
  const { setLastUpdate } = useLayoutContext()
  const { data: health } = useHealth()
  const { data: positionsData, isLoading: positionsLoading } = usePositions()

  // Update last update time when data changes
  useEffect(() => {
    if (health || positionsData) {
      setLastUpdate(new Date())
    }
  }, [health, positionsData, setLastUpdate])

  const positions = positionsData?.positions || []
  const activePositions = positions.filter((p) => p.state === 'ACTIVE')

  // Sample PnL data - in production this would come from API
  const pnlData = generateSamplePnLData(30)

  return (
    <div className="space-y-6">
      {/* System Status Bar */}
      <Card padding="sm">
        <div className="flex flex-col gap-4 md:flex-row md:items-center md:justify-between">
          <div className="flex flex-wrap items-center gap-4 md:gap-6">
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
              <span className="text-sm font-medium uppercase">
                System {health?.status || 'Unknown'}
              </span>
            </div>

            {/* Balance - placeholder */}
            <div className="flex items-center gap-2 text-sm">
              <span className="text-text-muted">Balance:</span>
              <span className="font-mono-numbers font-semibold">◎ 12.45 SOL</span>
            </div>

            {/* NAV - placeholder */}
            <div className="flex items-center gap-2 text-sm">
              <span className="text-text-muted">NAV:</span>
              <span className="font-mono-numbers font-semibold">$1,234.56</span>
            </div>
          </div>

          <div className="flex flex-wrap items-center gap-3 md:gap-4 text-sm">
            {/* Circuit Breaker Status */}
            <div className="flex items-center gap-2">
              <span className="text-text-muted hidden sm:inline">Circuit Breaker:</span>
              <span className="text-text-muted sm:hidden">CB:</span>
              <Badge
                variant={health?.circuit_breaker.trading_allowed ? 'success' : 'danger'}
              >
                {health?.circuit_breaker.trading_allowed ? 'Active' : 'Tripped'}
              </Badge>
            </div>

            {/* Queue Depth */}
            <div className="flex items-center gap-2">
              <span className="text-text-muted">Queue:</span>
              <span className="font-mono-numbers">{health?.queue_depth || 0}/1000</span>
            </div>

            {/* Uptime */}
            <div className="flex items-center gap-2">
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
              value="+$127.50"
              change="+10.3%"
              positive={true}
            />
            <MetricCard
              label="7d"
              value="+$892.30"
              change="+42.1%"
              positive={true}
            />
            <MetricCard
              label="30d"
              value="+$2,340.00"
              change="+89.5%"
              positive={true}
            />
          </div>
          <PnLChart data={pnlData} />
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
                  72%
                </div>
              </div>
              <div>
                <div className="text-xs md:text-sm text-text-muted">Avg Return</div>
                <div className="text-xl md:text-2xl font-semibold font-mono-numbers text-profit">
                  +8%
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
                <span className="font-mono-numbers">70%</span>
              </div>
              <div className="h-2 bg-background rounded-full overflow-hidden">
                <div className="h-full bg-shield w-[70%]" />
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
                  61%
                </div>
              </div>
              <div>
                <div className="text-xs md:text-sm text-text-muted">Avg Return</div>
                <div className="text-xl md:text-2xl font-semibold font-mono-numbers text-profit">
                  +23%
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
                <span className="font-mono-numbers">30%</span>
              </div>
              <div className="h-2 bg-background rounded-full overflow-hidden">
                <div className="h-full bg-spear w-[30%]" />
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
              status="healthy"
            />
            <HealthIndicator
              name="Jito"
              status="healthy"
            />
          </div>
        </CardContent>
      </Card>

      {/* Live Positions */}
      <Card padding="none">
        <div className="p-4 border-b border-border">
          <CardTitle>Live Positions</CardTitle>
        </div>
        {positionsLoading ? (
          <div className="p-8 text-center text-text-muted">Loading positions...</div>
        ) : positions.length === 0 ? (
          <div className="p-8 text-center text-text-muted">No active positions</div>
        ) : (
          <div className="overflow-x-auto">
          <Table>
            <TableHeader>
              <TableRow hoverable={false}>
                <TableHead>Token</TableHead>
                <TableHead>Strategy</TableHead>
                <TableHead>Size</TableHead>
                <TableHead>Entry</TableHead>
                <TableHead>Current</TableHead>
                <TableHead>PnL</TableHead>
                <TableHead>Status</TableHead>
                <TableHead></TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {positions.slice(0, 10).map((position) => (
                <TableRow key={position.trade_uuid}>
                  <TableCell>
                    <div className="font-semibold">
                      ${position.token_symbol || 'Unknown'}
                    </div>
                    <div className="text-xs text-text-muted">
                      {position.token_address.slice(0, 8)}...
                    </div>
                  </TableCell>
                  <TableCell>
                    <StrategyBadge strategy={position.strategy} />
                  </TableCell>
                  <TableCell mono>
                    {position.entry_amount_sol.toFixed(4)} SOL
                  </TableCell>
                  <TableCell mono>
                    {position.entry_price.toFixed(8)}
                  </TableCell>
                  <TableCell mono>
                    {position.current_price?.toFixed(8) || '-'}
                  </TableCell>
                  <TableCell mono>
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
                  <TableCell>
                    <StatusBadge status={position.state} />
                  </TableCell>
                  <TableCell>
                    {position.entry_tx_signature && (
                      <a
                        href={`https://solscan.io/tx/${position.entry_tx_signature}`}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="text-shield hover:text-shield-dark"
                      >
                        <ExternalLink className="w-4 h-4" />
                      </a>
                    )}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
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
  change: string
  positive: boolean
}) {
  return (
    <div className="bg-surface-light rounded-lg p-4">
      <div className="text-sm text-text-muted mb-1">{label}</div>
      <div className="text-2xl font-semibold font-mono-numbers">{value}</div>
      <div
        className={`text-sm font-mono-numbers ${
          positive ? 'text-profit' : 'text-loss'
        }`}
      >
        {change}
      </div>
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
