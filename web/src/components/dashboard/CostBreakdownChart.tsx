import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../ui/Table'
import type { CostAnalysisResponse } from '../../api'

interface CostBreakdownChartProps {
  data: CostAnalysisResponse
}

export function CostBreakdownChart({ data }: CostBreakdownChartProps) {
  return (
    <div className="space-y-4">
      {/* Cost by Type */}
      <div className="grid grid-cols-3 gap-4">
        {data.cost_by_type.map((cost) => (
          <div key={cost.type} className="bg-surface-light rounded-lg p-4">
            <div className="text-xs text-text-muted mb-1">
              {cost.type.replace('_', ' ').toUpperCase()}
            </div>
            <div className="text-lg font-semibold font-mono-numbers">
              {cost.total_sol.toFixed(4)} SOL
            </div>
            <div className="text-xs text-text-muted">
              Avg: {cost.average_sol.toFixed(6)} SOL
            </div>
            <div className="text-xs text-profit mt-1">
              {cost.percentage.toFixed(1)}% of total
            </div>
          </div>
        ))}
      </div>

      {/* Recent Trade Costs */}
      <Table>
        <TableHeader>
          <TableRow hoverable={false}>
            <TableHead>Token</TableHead>
            <TableHead className="text-right">Jito Tip</TableHead>
            <TableHead className="text-right">DEX Fee</TableHead>
            <TableHead className="text-right">Slippage</TableHead>
            <TableHead className="text-right">Total</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {data.per_trade_costs.slice(0, 5).map((trade) => (
            <TableRow key={trade.trade_uuid}>
              <TableCell>
                <div className="font-semibold">${trade.token_symbol || 'Unknown'}</div>
              </TableCell>
              <TableCell mono className="text-sm text-right">
                {trade.jito_tip_sol.toFixed(6)}
              </TableCell>
              <TableCell mono className="text-sm text-right">
                {trade.dex_fee_sol.toFixed(6)}
              </TableCell>
              <TableCell mono className="text-sm text-right">
                {trade.slippage_cost_sol.toFixed(6)}
              </TableCell>
              <TableCell mono className="text-sm text-right font-medium">
                {trade.total_cost_sol.toFixed(4)}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  )
}
