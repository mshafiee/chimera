import { Card, CardHeader, CardTitle, CardContent } from '../components/ui/Card'
import { Badge } from '../components/ui/Badge'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../components/ui/Table'
import { useState } from 'react'
import { MetricCard } from '../components/ui/MetricCard'
import { Webhook, Pulse, AlertCircle } from 'lucide-react'

// Mock data for now - this would come from API
interface WalletMonitoringState {
  address: string
  method: 'webhook' | 'polling'
  status: 'active' | 'inactive' | 'error'
  last_activity: string
  last_fetch: string | null
  failed_fetches: number
  success_rate: number
  next_fetch: string | null
}

export function WalletMonitoring() {
  // This would use useWalletMonitoringState() when API is ready
  const [walletStates] = useState<WalletMonitoringState[]>([
    {
      address: '7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU',
      method: 'webhook',
      status: 'active',
      last_activity: new Date(Date.now() - 5 * 60 * 1000).toISOString(),
      last_fetch: new Date(Date.now() - 2 * 60 * 1000).toISOString(),
      failed_fetches: 0,
      success_rate: 100,
      next_fetch: null,
    },
    {
      address: 'GThUX1Atko4tqhN2NaiTazWSeFWMuiUvfFnyJyUghFMJ',
      method: 'polling',
      status: 'active',
      last_activity: new Date(Date.now() - 15 * 60 * 1000).toISOString(),
      last_fetch: new Date(Date.now() - 10 * 60 * 1000).toISOString(),
      failed_fetches: 1,
      success_rate: 95,
      next_fetch: new Date(Date.now() + 5 * 60 * 1000).toISOString(),
    },
  ])

  const activeCount = walletStates.filter((w) => w.status === 'active').length
  const errorCount = walletStates.filter((w) => w.status === 'error').length
  const webhookCount = walletStates.filter((w) => w.method === 'webhook').length
  const pollingCount = walletStates.filter((w) => w.method === 'polling').length

  return (
    <div className="space-y-6">
      {/* Header */}
      <div>
        <h1 className="text-2xl font-bold">Wallet Monitoring</h1>
        <p className="text-text-muted text-sm">Per-wallet monitoring state and activity</p>
      </div>

      {/* Summary Metrics */}
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
        <MetricCard
          label="Active Monitors"
          value={activeCount}
          positive={activeCount > 0}
          icon={<Pulse className="w-4 h-4" />}
        />
        <MetricCard
          label="Webhook"
          value={webhookCount}
          icon={<Webhook className="w-4 h-4" />}
        />
        <MetricCard
          label="Polling"
          value={pollingCount}
          icon={<Pulse className="w-4 h-4" />}
        />
        <MetricCard
          label="Errors"
          value={errorCount}
          positive={errorCount === 0}
          icon={<AlertCircle className="w-4 h-4" />}
        />
      </div>

      {/* Wallet Monitoring Table */}
      <Card>
        <CardHeader>
          <CardTitle>Monitoring State</CardTitle>
        </CardHeader>
        <CardContent>
          <Table>
            <TableHeader>
              <TableRow hoverable={false}>
                <TableHead>Address</TableHead>
                <TableHead>Method</TableHead>
                <TableHead>Status</TableHead>
                <TableHead>Last Activity</TableHead>
                <TableHead>Last Fetch</TableHead>
                <TableHead className="text-right">Failed Fetches</TableHead>
                <TableHead className="text-right">Success Rate</TableHead>
                <TableHead>Next Fetch</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {walletStates.map((wallet) => (
                <TableRow key={wallet.address}>
                  <TableCell mono className="text-sm">
                    {wallet.address.slice(0, 8)}...{wallet.address.slice(-8)}
                  </TableCell>
                  <TableCell>
                    <Badge
                      variant={wallet.method === 'webhook' ? 'success' : 'warning'}
                      size="sm"
                    >
                      {wallet.method === 'webhook' ? (
                        <><Webhook className="w-3 h-3 inline mr-1" />Webhook</>
                      ) : (
                        <><Pulse className="w-3 h-3 inline mr-1" />Polling</>
                      )}
                    </Badge>
                  </TableCell>
                  <TableCell>
                    <Badge
                      variant={wallet.status === 'active' ? 'success' : wallet.status === 'error' ? 'danger' : 'default'}
                      size="sm"
                    >
                      {wallet.status}
                    </Badge>
                  </TableCell>
                  <TableCell className="text-sm text-text-muted">
                    {new Date(wallet.last_activity).toLocaleString()}
                  </TableCell>
                  <TableCell className="text-sm">
                    {wallet.last_fetch ? (
                      new Date(wallet.last_fetch).toLocaleString()
                    ) : (
                      <span className="text-text-muted">Never</span>
                    )}
                  </TableCell>
                  <TableCell mono className="text-sm text-right">
                    <span className={wallet.failed_fetches > 0 ? 'text-loss' : ''}>
                      {wallet.failed_fetches}
                    </span>
                  </TableCell>
                  <TableCell className="text-right">
                    <span className={wallet.success_rate >= 95 ? 'text-profit' : wallet.success_rate >= 80 ? 'text-spear' : 'text-loss'}>
                      {wallet.success_rate.toFixed(1)}%
                    </span>
                  </TableCell>
                  <TableCell className="text-sm text-text-muted">
                    {wallet.next_fetch ? (
                      new Date(wallet.next_fetch).toLocaleString()
                    ) : (
                      <span className="text-text-muted">N/A</span>
                    )}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </CardContent>
      </Card>

      {/* Monitoring Methods Info */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        <Card>
          <CardHeader>
            <CardTitle className="text-base">Webhook Monitoring</CardTitle>
          </CardHeader>
          <CardContent className="text-sm text-text-muted">
            <p className="mb-2">
              Real-time monitoring via webhook notifications. Wallets send signals immediately when trades occur.
            </p>
            <ul className="list-disc list-inside space-y-1">
              <li>Low latency</li>
              <li>Real-time updates</li>
              <li>Requires webhook registration</li>
            </ul>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="text-base">Polling Monitoring</CardTitle>
          </CardHeader>
          <CardContent className="text-sm text-text-muted">
            <p className="mb-2">
              Periodic polling of wallet activity. Operator checks for new transactions at regular intervals.
            </p>
            <ul className="list-disc list-inside space-y-1">
              <li>Higher latency</li>
              <li>Battery-friendly for wallets</li>
              <li>Fallback when webhook unavailable</li>
            </ul>
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
