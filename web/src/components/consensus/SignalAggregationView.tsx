import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../ui/Table'
import { Badge } from '../ui/Badge'
import type { SignalAggregationResponse } from '../../api'

interface SignalAggregationViewProps {
  data: SignalAggregationResponse
}

const ACTION_CONFIG = {
  BUY: { variant: 'success' as const, label: 'BUY' },
  SELL: { variant: 'danger' as const, label: 'SELL' },
  HOLD: { variant: 'default' as const, label: 'HOLD' },
  SKIP: { variant: 'warning' as const, label: 'SKIP' },
}

export function SignalAggregationView({ data }: SignalAggregationViewProps) {
  return (
    <div className="space-y-4">
      {/* Summary */}
      <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Total Signals</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.total_signals ?? 0}
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Unique Tokens</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.unique_tokens ?? 0}
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Aggregated</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.aggregated_signals?.length ?? 0}
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Aggregation Latency</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {(data.aggregation_latency_ms ?? 0).toFixed(0)}ms
          </div>
        </div>
      </div>

      {/* Aggregated Signals Table */}
      <Table>
        <TableHeader>
          <TableRow hoverable={false}>
            <TableHead>Token</TableHead>
            <TableHead className="text-right">Signals</TableHead>
            <TableHead className="text-right">Unique Wallets</TableHead>
            <TableHead className="text-right">Consensus Score</TableHead>
            <TableHead>Recommended</TableHead>
            <TableHead>Confidence</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {data.aggregated_signals?.map((signal, index) => {
            const actionConfig = ACTION_CONFIG[signal.recommended_action]
            return (
              <TableRow key={`${signal.token_address}-${index}`}>
                <TableCell>
                  <div className="font-semibold">
                    ${signal.token_symbol || 'Unknown'}
                  </div>
                  <div className="text-xs text-text-muted">
                    {signal.token_address?.slice(0, 8) || 'N/A'}...
                  </div>
                </TableCell>
                <TableCell mono className="text-sm text-right">
                  {signal.signal_count}
                </TableCell>
                <TableCell mono className="text-sm text-right">
                  {signal.unique_wallets}
                </TableCell>
                <TableCell mono className="text-sm text-right">
                  <span className={(signal.consensus_score ?? 0) >= 0.7 ? 'text-profit' : (signal.consensus_score ?? 0) >= 0.5 ? 'text-spear' : 'text-loss'}>
                    {(signal.consensus_score ?? 0).toFixed(2)}
                  </span>
                </TableCell>
                <TableCell>
                  <Badge variant={actionConfig.variant} size="sm">
                    {actionConfig.label}
                  </Badge>
                </TableCell>
                <TableCell>
                  <div className="flex items-center gap-2">
                    <div className="w-16 h-2 bg-surface rounded-full overflow-hidden">
                      <div
                        className={`h-full ${
                          (signal.confidence ?? 0) >= 0.7 ? 'bg-profit' : (signal.confidence ?? 0) >= 0.5 ? 'bg-spear' : 'bg-loss'
                        }`}
                        style={{ width: `${(signal.confidence ?? 0) * 100}%` }}
                      />
                    </div>
                    <span className="text-xs text-text-muted">
                      {((signal.confidence ?? 0) * 100).toFixed(0)}%
                    </span>
                  </div>
                </TableCell>
              </TableRow>
            )
          })}
        </TableBody>
      </Table>
    </div>
  )
}
