import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../ui/Table'
import { Badge } from '../ui/Badge'
import type { StopLossMetricsResponse } from '../../api'

interface StopLossAnalyticsProps {
  data: StopLossMetricsResponse
}

export function StopLossAnalytics({ data }: StopLossAnalyticsProps) {
  return (
    <div className="space-y-6">
      {/* Summary Metrics */}
      <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Activation Rate</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {(data.activation_rate * 100).toFixed(1)}%
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Total Activations</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.total_activations}
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Loss Prevented</div>
          <div className="text-xl font-semibold font-mono-numbers text-profit">
            {data.loss_prevented_sol.toFixed(4)} SOL
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Avg Loss Prevented</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.average_loss_prevented_sol.toFixed(4)} SOL
          </div>
        </div>
      </div>

      {/* By Strategy */}
      <div>
        <h3 className="text-sm font-medium mb-3">By Strategy</h3>
        <div className="grid grid-cols-2 gap-4">
          {data.activations_by_strategy.map((strategy) => (
            <div key={strategy.strategy} className="bg-surface-light rounded-lg p-4">
              <div className="flex items-center justify-between mb-2">
                <Badge variant={strategy.strategy === 'SHIELD' ? 'success' : 'warning'} size="sm">
                  {strategy.strategy}
                </Badge>
                <span className="text-xs text-text-muted">{strategy.activations} activations</span>
              </div>
              <div className="text-lg font-semibold font-mono-numbers text-profit">
                {strategy.loss_prevented_sol.toFixed(4)} SOL
              </div>
              <div className="text-xs text-text-muted">Loss Prevented</div>
            </div>
          ))}
        </div>
      </div>

      {/* Recent Activations */}
      {data.recent_activations.length > 0 && (
        <div>
          <h3 className="text-sm font-medium mb-3">Recent Activations</h3>
          <Table>
            <TableHeader>
              <TableRow hoverable={false}>
                <TableHead>Time</TableHead>
                <TableHead>Token</TableHead>
                <TableHead className="text-right">Entry Price</TableHead>
                <TableHead className="text-right">Stop Price</TableHead>
                <TableHead className="text-right">Loss Prevented</TableHead>
                <TableHead>Strategy</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {data.recent_activations.slice(0, 10).map((activation) => (
                <TableRow key={activation.trade_uuid}>
                  <TableCell className="text-sm text-text-muted">
                    {new Date(activation.timestamp).toLocaleString()}
                  </TableCell>
                  <TableCell>
                    <div className="font-semibold">
                      ${activation.token_symbol || 'Unknown'}
                    </div>
                  </TableCell>
                  <TableCell mono className="text-sm text-right">
                    {activation.entry_price.toFixed(8)}
                  </TableCell>
                  <TableCell mono className="text-sm text-right">
                    {activation.stop_price.toFixed(8)}
                  </TableCell>
                  <TableCell mono className="text-sm text-right text-profit">
                    {activation.loss_prevented_sol.toFixed(4)} SOL
                  </TableCell>
                  <TableCell>
                    <Badge variant={activation.strategy === 'SHIELD' ? 'success' : 'warning'} size="sm">
                      {activation.strategy}
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
