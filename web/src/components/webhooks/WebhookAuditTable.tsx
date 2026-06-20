import { useState } from 'react'
import { Badge } from '../ui/Badge'
import type { WebhookAuditLog, WebhookAction, WebhookStatus } from '../../api'
import { ChevronDown, ChevronRight, AlertCircle, Clock } from 'lucide-react'

interface WebhookAuditTableProps {
  logs: WebhookAuditLog[] | null | undefined
  isLoading: boolean
}

export function WebhookAuditTable({ logs, isLoading }: WebhookAuditTableProps) {
  const [expandedRow, setExpandedRow] = useState<number | null>(null)

  const getActionBadge = (action: WebhookAction) => {
    const variants: Record<WebhookAction, 'success' | 'warning' | 'danger' | 'default'> = {
      register: 'success',
      update: 'default',
      delete: 'danger',
      toggle: 'warning',
      health_check: 'default',
      reconcile: 'default',
    }
    return <Badge variant={variants[action]} size="sm">{action}</Badge>
  }

  const getStatusBadge = (status: WebhookStatus) => {
    const variants: Record<WebhookStatus, 'success' | 'warning' | 'danger' | 'default'> = {
      success: 'success',
      failed: 'danger',
      pending: 'warning',
      retry: 'warning',
    }
    return <Badge variant={variants[status]} size="sm">{status}</Badge>
  }

  const formatDuration = (ms: number | null) => {
    if (ms === null) return '-'
    if (ms < 1000) return `${ms}ms`
    return `${(ms / 1000).toFixed(2)}s`
  }

  const formatDate = (dateString: string) => {
    return new Date(dateString).toLocaleString()
  }

  const truncateAddress = (address: string) => {
    if (address.length <= 16) return address
    return `${address.slice(0, 8)}...${address.slice(-8)}`
  }

  if (isLoading) {
    return (
      <div className="flex items-center justify-center py-12">
        <div className="flex items-center gap-3 text-text-muted">
          <div className="animate-spin w-5 h-5 border-2 border-current border-t-transparent rounded-full" />
          <span>Loading audit log...</span>
        </div>
      </div>
    )
  }

  if (!logs || logs.length === 0) {
    return (
      <div className="text-center text-text-muted py-8">
        <Clock className="w-8 h-8 mx-auto mb-2 opacity-50" />
        No audit log entries found
      </div>
    )
  }

  return (
    <div className="overflow-x-auto">
      <table className="w-full">
        <thead>
          <tr className="border-b border-border">
            <th className="text-left py-3 px-4 text-sm font-medium text-text-muted w-8" />
            <th className="text-left py-3 px-4 text-sm font-medium text-text-muted">Timestamp</th>
            <th className="text-left py-3 px-4 text-sm font-medium text-text-muted">Wallet</th>
            <th className="text-left py-3 px-4 text-sm font-medium text-text-muted">Action</th>
            <th className="text-left py-3 px-4 text-sm font-medium text-text-muted">Status</th>
            <th className="text-left py-3 px-4 text-sm font-medium text-text-muted">Duration</th>
            <th className="text-left py-3 px-4 text-sm font-medium text-text-muted">Webhook ID</th>
          </tr>
        </thead>
        <tbody>
          {logs.map((log) => (
            <>
              <tr
                key={log.id}
                className="border-b border-border hover:bg-surface-light transition-colors cursor-pointer"
                onClick={() => setExpandedRow(expandedRow === log.id ? null : log.id)}
              >
                <td className="py-3 px-4">
                  {log.error_message || log.details ? (
                    expandedRow === log.id ? (
                      <ChevronDown className="w-4 h-4 text-text-muted" />
                    ) : (
                      <ChevronRight className="w-4 h-4 text-text-muted" />
                    )
                  ) : null}
                </td>
                <td className="py-3 px-4 text-sm font-mono-numbers text-text-muted">
                  {formatDate(log.created_at)}
                </td>
                <td className="py-3 px-4 text-sm font-mono-numbers">
                  {truncateAddress(log.wallet_address)}
                </td>
                <td className="py-3 px-4">
                  {getActionBadge(log.action)}
                </td>
                <td className="py-3 px-4">
                  {getStatusBadge(log.status)}
                </td>
                <td className="py-3 px-4 text-sm font-mono-numbers text-text-muted">
                  {formatDuration(log.duration_ms)}
                </td>
                <td className="py-3 px-4 text-sm font-mono-numbers text-text-muted">
                  {log.webhook_id ? truncateAddress(log.webhook_id) : '-'}
                </td>
              </tr>
              {expandedRow === log.id && (log.error_message || log.details) && (
                <tr className="bg-surface-light">
                  <td colSpan={7} className="py-3 px-4">
                    <div className="space-y-2 text-sm">
                      {log.details && (
                        <div>
                          <span className="font-medium text-text-muted">Details: </span>
                          <span className="text-text">{log.details}</span>
                        </div>
                      )}
                      {log.error_message && (
                        <div className="flex items-start gap-2 text-loss">
                          <AlertCircle className="w-4 h-4 mt-0.5 flex-shrink-0" />
                          <span>{log.error_message}</span>
                        </div>
                      )}
                    </div>
                  </td>
                </tr>
              )}
            </>
          ))}
        </tbody>
      </table>
    </div>
  )
}
