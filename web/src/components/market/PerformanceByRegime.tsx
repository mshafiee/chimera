import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../ui/Table'
import { Badge } from '../ui/Badge'
import type { PerformanceByRegime } from '../../api'

interface PerformanceByRegimeProps {
  data: PerformanceByRegime[]
}

export function PerformanceByRegime({ data }: PerformanceByRegimeProps) {
  const regimeLabels: Record<string, string> = {
    bull: 'Bull',
    bear: 'Bear',
    neutral: 'Neutral',
    volatile: 'Volatile',
  }

  return (
    <Table>
      <TableHeader>
        <TableRow hoverable={false}>
          <TableHead>Regime</TableHead>
          <TableHead className="text-right">Trades</TableHead>
          <TableHead className="text-right">Win Rate</TableHead>
          <TableHead className="text-right">Avg Return</TableHead>
          <TableHead className="text-right">Total PnL</TableHead>
          <TableHead className="text-right">Sharpe</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {data.map((perf) => (
          <TableRow key={perf.regime}>
            <TableCell>
              <Badge
                variant={
                  perf.regime === 'bull' ? 'success' :
                  perf.regime === 'bear' ? 'danger' :
                  perf.regime === 'volatile' ? 'warning' : 'default'
                }
              >
                {regimeLabels[perf.regime] || perf.regime}
              </Badge>
            </TableCell>
            <TableCell mono className="text-right text-sm">
              {perf.total_trades}
            </TableCell>
            <TableCell mono className="text-right">
              <span className={perf.win_rate >= 50 ? 'text-profit' : 'text-loss'}>
                {perf.win_rate.toFixed(1)}%
              </span>
            </TableCell>
            <TableCell mono className="text-right">
              <span className={perf.avg_return >= 0 ? 'text-profit' : 'text-loss'}>
                {perf.avg_return >= 0 ? '+' : ''}${perf.avg_return.toFixed(2)}
              </span>
            </TableCell>
            <TableCell mono className="text-right">
              <span className={perf.total_pnl >= 0 ? 'text-profit' : 'text-loss'}>
                {perf.total_pnl >= 0 ? '+' : ''}${perf.total_pnl.toFixed(2)}
              </span>
            </TableCell>
            <TableCell mono className="text-right">
              <span className={perf.sharpe_ratio >= 1 ? 'text-profit' : perf.sharpe_ratio >= 0 ? 'text-spear' : 'text-loss'}>
                {perf.sharpe_ratio.toFixed(2)}
              </span>
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  )
}
