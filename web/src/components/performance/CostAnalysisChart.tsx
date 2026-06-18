import { PieChart, Pie, Cell, ResponsiveContainer, Legend, Tooltip } from 'recharts'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../ui/Table'
import { Badge } from '../ui/Badge'
import type { CostAnalysisResponse } from '../../api'

interface CostAnalysisChartProps {
  data: CostAnalysisResponse
}

const COST_COLORS = {
  jito_tip: '#3b82f6',
  dex_fee: '#22c55e',
  slippage: '#f97316',
}

export function CostAnalysisChart({ data }: CostAnalysisChartProps) {
  const pieData = data.cost_by_type.map((item) => ({
    name: item.type.replace('_', ' '),
    value: item.total_sol,
    percentage: item.percentage,
  }))

  const CustomTooltip = ({ active, payload }: any) => {
    if (active && payload && payload.length) {
      return (
        <div className="bg-surface border border-border rounded-lg p-3 shadow-lg">
          <p className="text-sm font-medium">{payload[0].name}</p>
          <p className="text-xs text-text-muted">Total: {payload[0].value.toFixed(4)} SOL</p>
          <p className="text-xs text-text-muted">Percentage: {payload[0].payload.percentage.toFixed(1)}%</p>
        </div>
      )
    }
    return null
  }

  return (
    <div className="space-y-6">
      {/* Summary */}
      <div className="grid grid-cols-3 gap-4">
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Total Costs</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.total_costs.toFixed(4)} SOL
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Avg Per Trade</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.avg_cost_per_trade.toFixed(4)} SOL
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Trades Analyzed</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.per_trade_costs.length}
          </div>
        </div>
      </div>

      {/* Cost Breakdown Chart */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
        {/* Pie Chart */}
        <div>
          <h3 className="text-sm font-medium mb-3">Cost Distribution</h3>
          <div className="h-64">
            <ResponsiveContainer width="100%" height="100%">
              <PieChart>
                <Pie
                  data={pieData}
                  cx="50%"
                  cy="50%"
                  labelLine={false}
                  label={(entry) => `${entry.name}: ${entry.percentage.toFixed(1)}%`}
                  outerRadius={80}
                  dataKey="value"
                >
                  {pieData.map((entry, index) => (
                    <Cell
                      key={`cell-${index}`}
                      fill={COST_COLORS[entry.name as keyof typeof COST_COLORS] || '#6b7280'}
                    />
                  ))}
                </Pie>
                <Tooltip content={<CustomTooltip />} />
              </PieChart>
            </ResponsiveContainer>
          </div>
        </div>

        {/* Cost by Type Table */}
        <div>
          <h3 className="text-sm font-medium mb-3">Cost by Type</h3>
          <Table>
            <TableHeader>
              <TableRow hoverable={false}>
                <TableHead>Type</TableHead>
                <TableHead className="text-right">Total (SOL)</TableHead>
                <TableHead className="text-right">Avg (SOL)</TableHead>
                <TableHead className="text-right">%</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {data.cost_by_type.map((item) => (
                <TableRow key={item.type}>
                  <TableCell className="font-medium">
                    {item.type.replace('_', ' ').replace(/\b\w/g, l => l.toUpperCase())}
                  </TableCell>
                  <TableCell mono className="text-sm text-right">
                    {item.total_sol.toFixed(4)}
                  </TableCell>
                  <TableCell mono className="text-sm text-right">
                    {item.average_sol.toFixed(6)}
                  </TableCell>
                  <TableCell className="text-right">
                    {item.percentage.toFixed(1)}%
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      </div>

      {/* Optimization Opportunities */}
      {data.optimization_opportunities.length > 0 && (
        <div>
          <h3 className="text-sm font-medium mb-3">Optimization Opportunities</h3>
          <div className="space-y-2">
            {data.optimization_opportunities.map((opp, index) => (
              <div key={index} className="bg-surface-light rounded-lg p-4">
                <div className="flex items-center justify-between">
                  <div>
                    <div className="font-medium text-sm">{opp.type}</div>
                    <div className="text-xs text-text-muted mt-1">{opp.description}</div>
                  </div>
                  <div className="text-right">
                    <div className="text-sm font-semibold text-profit">
                      Save {opp.potential_savings_sol.toFixed(4)} SOL
                    </div>
                    <div className="text-xs text-text-muted">
                      {opp.current_value} → {opp.recommended_value}
                    </div>
                  </div>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Per-Trade Costs (Top 10) */}
      {data.per_trade_costs.length > 0 && (
        <div>
          <h3 className="text-sm font-medium mb-3">Per-Trade Costs (Recent)</h3>
          <Table>
            <TableHeader>
              <TableRow hoverable={false}>
                <TableHead>Token</TableHead>
                <TableHead className="text-right">Jito Tip</TableHead>
                <TableHead className="text-right">DEX Fee</TableHead>
                <TableHead className="text-right">Slippage</TableHead>
                <TableHead className="text-right">Total</TableHead>
                <TableHead className="text-right">Exec Time</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {data.per_trade_costs.slice(0, 10).map((trade) => (
                <TableRow key={trade.trade_uuid}>
                  <TableCell>
                    <div className="font-semibold">
                      ${trade.token_symbol || 'Unknown'}
                    </div>
                    <div className="text-xs text-text-muted">
                      {new Date(trade.timestamp).toLocaleString()}
                    </div>
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
                  <TableCell mono className="text-sm text-right">
                    {trade.execution_time_ms.toFixed(0)}ms
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
