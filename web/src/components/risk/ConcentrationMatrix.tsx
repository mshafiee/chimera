import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../ui/Table'
import { Badge } from '../ui/Badge'
import type { ConcentrationData } from '../../api'

interface ConcentrationMatrixProps {
  data: ConcentrationData
}

export function ConcentrationMatrix({ data }: ConcentrationMatrixProps) {
  return (
    <div className="space-y-6">
      {/* Token Concentration */}
      <div>
        <h3 className="text-sm font-medium mb-3">Concentration by Token</h3>
        <Table>
          <TableHeader>
            <TableRow hoverable={false}>
              <TableHead>Token</TableHead>
              <TableHead className="text-right">Positions</TableHead>
              <TableHead className="text-right">Value (SOL)</TableHead>
              <TableHead className="text-right">Percentage</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {data.by_token.slice(0, 10).map((token) => (
              <TableRow key={token.token_address}>
                <TableCell>
                  <div className="font-semibold">
                    ${token.token_symbol || 'Unknown'}
                  </div>
                  <div className="text-xs text-text-muted">
                    {token.token_address.slice(0, 8)}...
                  </div>
                </TableCell>
                <TableCell mono className="text-sm text-right">
                  {token.position_count}
                </TableCell>
                <TableCell mono className="text-sm text-right">
                  {token.total_value_sol.toFixed(4)}
                </TableCell>
                <TableCell className="text-right">
                  <Badge
                    variant={token.percentage > 20 ? 'danger' : token.percentage > 10 ? 'warning' : 'success'}
                    size="sm"
                  >
                    {token.percentage.toFixed(1)}%
                  </Badge>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>

      {/* Sector Concentration */}
      {data.by_sector && data.by_sector.length > 0 && (
        <div>
          <h3 className="text-sm font-medium mb-3">Concentration by Sector</h3>
          <Table>
            <TableHeader>
              <TableRow hoverable={false}>
                <TableHead>Sector</TableHead>
                <TableHead className="text-right">Positions</TableHead>
                <TableHead className="text-right">Value (SOL)</TableHead>
                <TableHead className="text-right">Percentage</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {data.by_sector.map((sector) => (
                <TableRow key={sector.sector}>
                  <TableCell className="font-medium">{sector.sector}</TableCell>
                  <TableCell mono className="text-sm text-right">
                    {sector.position_count}
                  </TableCell>
                  <TableCell mono className="text-sm text-right">
                    {sector.total_value_sol.toFixed(4)}
                  </TableCell>
                  <TableCell className="text-right">
                    <Badge
                      variant={sector.percentage > 30 ? 'danger' : sector.percentage > 15 ? 'warning' : 'success'}
                      size="sm"
                    >
                      {sector.percentage.toFixed(1)}%
                    </Badge>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}

      {/* Concentration Metrics */}
      <div className="grid grid-cols-2 gap-4 pt-4 border-t border-border">
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">Max Concentration</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.max_concentration_percent.toFixed(1)}%
          </div>
        </div>
        <div className="bg-surface-light rounded-lg p-4">
          <div className="text-xs text-text-muted mb-1">HHI (Herfindahl Index)</div>
          <div className="text-xl font-semibold font-mono-numbers">
            {data.hhi.toFixed(3)}
          </div>
        </div>
      </div>
    </div>
  )
}
