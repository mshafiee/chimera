import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../ui/Table'
import { Badge } from '../ui/Badge'
import type { ProfitTargetMetricsResponse } from '../../api'

interface ProfitTargetAnalyticsProps {
  data: ProfitTargetMetricsResponse
}

export function ProfitTargetAnalytics({ data }: ProfitTargetAnalyticsProps) {
  return (
    <div className="space-y-6">
      {/* Summary Metrics */}
      <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Hit Rate</div>
          <div className={`text-xl font-semibold font-mono-numbers ${
            data.hit_rate >= 0.5 ? 'text-profit' : 'text-loss'
          }`}>
            {(data.hit_rate * 100).toFixed(1)}%
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Total Hits</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.total_hits} / {data.total_targets}
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Trailing Stop Acts</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.trailing_stop_activations}
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Avg Gain</div>
          <div className="text-xl font-semibold font-mono-numbers text-profit">
            {data.average_realized_gain_sol.toFixed(4)} SOL
          </div>
        </div>
      </div>

      {/* By Strategy */}
      <div>
        <h3 className="text-sm font-medium mb-3">By Strategy</h3>
        <div className="grid grid-cols-2 gap-4">
          {data.targets_by_strategy.map((strategy) => (
            <div key={strategy.strategy} className="bg-surface-light rounded-lg p-4">
              <div className="flex items-center justify-between mb-2">
                <Badge variant={strategy.strategy === 'SHIELD' ? 'success' : 'warning'} size="sm">
                  {strategy.strategy}
                </Badge>
                <span className="text-xs text-text-muted">
                  {strategy.total_hits} hits
                </span>
              </div>
              <div className="text-lg font-semibold font-mono-numbers">
                <span className={strategy.hit_rate >= 0.5 ? 'text-profit' : 'text-loss'}>
                  {(strategy.hit_rate * 100).toFixed(1)}%
                </span>
                <span className="text-xs text-text-muted ml-2">
                  hit rate
                </span>
              </div>
              <div className="text-sm text-text-muted">
                Avg gain: {strategy.average_gain_sol.toFixed(4)} SOL
              </div>
            </div>
          ))}
        </div>
      </div>

      {/* Recent Hits */}
      {data.recent_hits.length > 0 && (
        <div>
          <h3 className="text-sm font-medium mb-3">Recent Target Hits</h3>
          <Table>
            <TableHeader>
              <TableRow hoverable={false}>
                <TableHead>Time</TableHead>
                <TableHead>Token</TableHead>
                <TableHead className="text-right">Target Level</TableHead>
                <TableHead className="text-right">Realized Gain</TableHead>
                <TableHead>Strategy</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {data.recent_hits.slice(0, 10).map((hit) => (
                <TableRow key={hit.trade_uuid}>
                  <TableCell className="text-sm text-text-muted">
                    {new Date(hit.timestamp).toLocaleString()}
                  </TableCell>
                  <TableCell>
                    <div className="font-semibold">
                      ${hit.token_symbol || 'Unknown'}
                    </div>
                  </TableCell>
                  <TableCell mono className="text-sm text-right">
                    {hit.target_level}x
                  </TableCell>
                  <TableCell mono className="text-sm text-right text-profit">
                    {hit.realized_gain_sol.toFixed(4)} SOL
                  </TableCell>
                  <TableCell>
                    <Badge variant={hit.strategy === 'SHIELD' ? 'success' : 'warning'} size="sm">
                      {hit.strategy}
                    </Badge>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}
    </div>
  )
}
