import { useState } from 'react'
import { AlertTriangle, AlertCircle, Info, Clock, User, FileText } from 'lucide-react'
import { Card } from '../components/ui/Card'
import { Button } from '../components/ui/Button'
import { Badge } from '../components/ui/Badge'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../components/ui/Table'
import { useDeadLetterQueue, useConfigAudit } from '../api'

type TabType = 'dead-letter' | 'config-audit'
type SeverityFilter = 'all' | 'critical' | 'warning' | 'info'

export function Incidents() {
  const [activeTab, setActiveTab] = useState<TabType>('dead-letter')
  const [severityFilter, setSeverityFilter] = useState<SeverityFilter>('all')

  const { data: dlqData, isLoading: dlqLoading } = useDeadLetterQueue()
  const { data: auditData, isLoading: auditLoading } = useConfigAudit()

  return (
    <div className="space-y-6">
      {/* Tabs */}
      <div className="flex items-center gap-4 border-b border-border">
        <button
          onClick={() => setActiveTab('dead-letter')}
          className={`px-4 py-3 text-sm font-medium border-b-2 transition-colors ${
            activeTab === 'dead-letter'
              ? 'border-shield text-shield'
              : 'border-transparent text-text-muted hover:text-text'
          }`}
        >
          <div className="flex items-center gap-2">
            <AlertTriangle className="w-4 h-4" />
            Dead Letter Queue
            {(dlqData?.total || 0) > 0 && (
              <Badge variant="danger" size="sm">
                {dlqData?.total}
              </Badge>
            )}
          </div>
        </button>
        <button
          onClick={() => setActiveTab('config-audit')}
          className={`px-4 py-3 text-sm font-medium border-b-2 transition-colors ${
            activeTab === 'config-audit'
              ? 'border-shield text-shield'
              : 'border-transparent text-text-muted hover:text-text'
          }`}
        >
          <div className="flex items-center gap-2">
            <FileText className="w-4 h-4" />
            Config Audit Log
          </div>
        </button>
      </div>

      {/* Content */}
      {activeTab === 'dead-letter' ? (
        <DeadLetterTab
          data={dlqData}
          isLoading={dlqLoading}
          severityFilter={severityFilter}
          onSeverityChange={setSeverityFilter}
        />
      ) : (
        <ConfigAuditTab data={auditData} isLoading={auditLoading} />
      )}
    </div>
  )
}

function DeadLetterTab({
  data,
  isLoading,
  severityFilter,
  onSeverityChange,
}: {
  data: { items: Incident[]; total: number } | undefined
  isLoading: boolean
  severityFilter: SeverityFilter
  onSeverityChange: (filter: SeverityFilter) => void
}) {
  const items = data?.items || []

  // Get severity from reason
  const getSeverity = (reason: string): 'critical' | 'warning' | 'info' => {
    if (reason.includes('MAX_RETRIES') || reason.includes('PARSE_ERROR')) {
      return 'critical'
    }
    if (reason.includes('QUEUE_FULL') || reason.includes('VALIDATION')) {
      return 'warning'
    }
    return 'info'
  }

  const filteredItems =
    severityFilter === 'all'
      ? items
      : items.filter((item: Incident) => getSeverity(item.reason) === severityFilter)

  return (
    <div className="space-y-4">
      {/* Filters */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <span className="text-sm text-text-muted">Severity:</span>
          <div className="flex rounded-lg border border-border overflow-hidden">
            {(['all', 'critical', 'warning', 'info'] as SeverityFilter[]).map((filter) => (
              <button
                key={filter}
                onClick={() => onSeverityChange(filter)}
                className={`px-3 py-1.5 text-xs font-medium transition-colors capitalize ${
                  severityFilter === filter
                    ? 'bg-shield text-background'
                    : 'bg-surface text-text-muted hover:text-text hover:bg-surface-light'
                }`}
              >
                {filter}
              </button>
            ))}
          </div>
        </div>

        <div className="text-sm text-text-muted">
          {filteredItems.length} incident{filteredItems.length !== 1 ? 's' : ''}
        </div>
      </div>

      {/* Table */}
      <Card padding="none">
        {isLoading ? (
          <div className="p-8 text-center text-text-muted">Loading incidents...</div>
        ) : filteredItems.length === 0 ? (
          <div className="p-8 text-center text-text-muted">
            <AlertCircle className="w-12 h-12 mx-auto mb-4 opacity-50" />
            <div>No incidents found</div>
            <div className="text-sm mt-1">The dead letter queue is empty</div>
          </div>
        ) : (
          <Table>
            <TableHeader>
              <TableRow hoverable={false}>
                <TableHead>Severity</TableHead>
                <TableHead>Time</TableHead>
                <TableHead>Reason</TableHead>
                <TableHead>Trade UUID</TableHead>
                <TableHead>Error Details</TableHead>
                <TableHead>Actions</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {filteredItems.map((item: any) => {
                const severity = getSeverity(item.reason)
                return (
                  <TableRow key={item.id}>
                    <TableCell>
                      <SeverityIcon severity={severity} />
                    </TableCell>
                    <TableCell>
                      <div className="text-sm">{formatTime(item.received_at)}</div>
                      <div className="text-xs text-text-muted">
                        {formatDate(item.received_at)}
                      </div>
                    </TableCell>
                    <TableCell>
                      <Badge
                        variant={
                          severity === 'critical'
                            ? 'danger'
                            : severity === 'warning'
                            ? 'warning'
                            : 'info'
                        }
                      >
                        {item.reason}
                      </Badge>
                    </TableCell>
                    <TableCell>
                      <code className="text-xs text-shield">
                        {item.trade_uuid || '-'}
                      </code>
                    </TableCell>
                    <TableCell>
                      <div className="text-sm text-text-muted max-w-[300px] truncate">
                        {item.error_details || '-'}
                      </div>
                    </TableCell>
                    <TableCell>
                      {item.can_retry && (
                        <Button variant="ghost" size="sm">
                          Retry
                        </Button>
                      )}
                    </TableCell>
                  </TableRow>
                )
              })}
            </TableBody>
          </Table>
        )}
      </Card>
    </div>
  )
}

function ConfigAuditTab({ 
  data, 
  isLoading 
}: { 
  data: { items: ConfigAudit[]; total: number } | undefined
  isLoading: boolean 
}) {
  const items = data?.items || []

  return (
    <Card padding="none">
      {isLoading ? (
        <div className="p-8 text-center text-text-muted">Loading audit log...</div>
      ) : items.length === 0 ? (
        <div className="p-8 text-center text-text-muted">
          <FileText className="w-12 h-12 mx-auto mb-4 opacity-50" />
          <div>No audit entries found</div>
        </div>
      ) : (
        <Table>
          <TableHeader>
            <TableRow hoverable={false}>
              <TableHead>Time</TableHead>
              <TableHead>Key</TableHead>
              <TableHead>Change</TableHead>
              <TableHead>Changed By</TableHead>
              <TableHead>Reason</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {items.map((item: ConfigAudit) => (
              <TableRow key={item.id}>
                <TableCell>
                  <div className="flex items-center gap-2">
                    <Clock className="w-4 h-4 text-text-muted" />
                    <div>
                      <div className="text-sm">{formatTime(item.changed_at)}</div>
                      <div className="text-xs text-text-muted">
                        {formatDate(item.changed_at)}
                      </div>
                    </div>
                  </div>
                </TableCell>
                <TableCell>
                  <code className="text-xs bg-surface-light px-2 py-1 rounded">
                    {item.key}
                  </code>
                </TableCell>
                <TableCell>
                  <div className="flex items-center gap-2 text-sm">
                    <span className="text-loss line-through">
                      {item.old_value || '(none)'}
                    </span>
                    <span className="text-text-muted">→</span>
                    <span className="text-profit">{item.new_value}</span>
                  </div>
                </TableCell>
                <TableCell>
                  <div className="flex items-center gap-2">
                    <User className="w-4 h-4 text-text-muted" />
                    <span className="text-sm">{item.changed_by}</span>
                  </div>
                </TableCell>
                <TableCell>
                  <span className="text-sm text-text-muted">
                    {item.change_reason || '-'}
                  </span>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
    </Card>
  )
}

function SeverityIcon({ severity }: { severity: 'critical' | 'warning' | 'info' }) {
  switch (severity) {
    case 'critical':
      return (
        <div className="flex items-center gap-2">
          <AlertTriangle className="w-4 h-4 text-loss" />
          <span className="text-xs text-loss uppercase font-medium">Critical</span>
        </div>
      )
    case 'warning':
      return (
        <div className="flex items-center gap-2">
          <AlertCircle className="w-4 h-4 text-spear" />
          <span className="text-xs text-spear uppercase font-medium">Warning</span>
        </div>
      )
    case 'info':
      return (
        <div className="flex items-center gap-2">
          <Info className="w-4 h-4 text-shield" />
          <span className="text-xs text-shield uppercase font-medium">Info</span>
        </div>
      )
  }
}

function formatTime(dateStr: string): string {
  const date = new Date(dateStr)
  return date.toLocaleTimeString('en-US', {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  })
}

function formatDate(dateStr: string): string {
  const date = new Date(dateStr)
  return date.toLocaleDateString('en-US', {
    month: 'short',
    day: 'numeric',
  })
}
