import { MetricCard } from '../ui/MetricCard'
import type { ResourceUsageResponse } from '../../api'
import { Cpu, HardDrive, Wifi, Memory } from 'lucide-react'

interface ResourceUsageCardProps {
  data: ResourceUsageResponse
}

export function ResourceUsageCard({ data }: ResourceUsageCardProps) {
  const getStatusVariant = (status: string) => {
    switch (status) {
      case 'normal': return 'success'
      case 'warning': return 'warning'
      case 'critical': return 'danger'
      default: return 'default'
    }
  }

  return (
    <div className="space-y-6">
      {/* CPU */}
      <MetricCard
        label="CPU Usage"
        value={`${data.cpu.percentage.toFixed(0)}%`}
        positive={data.cpu.status === 'normal'}
        icon={<Cpu className="w-4 h-4" />}
      />

      {/* Memory */}
      <MetricCard
        label="Memory Usage"
        value={`${data.memory.percentage.toFixed(0)}%`}
        positive={data.memory.status === 'normal'}
        icon={<Memory className="w-4 h-4" />}
      />

      {/* Disk */}
      <MetricCard
        label="Disk Usage"
        value={`${data.disk.percentage.toFixed(0)}%`}
        positive={data.disk.status === 'normal'}
        icon={<HardDrive className="w-4 h-4" />}
      />

      {/* Network */}
      <div className="bg-surface-light rounded-lg p-4">
        <div className="flex items-center gap-2 mb-3">
          <Wifi className="w-4 h-4 text-text-muted" />
          <span className="text-sm font-medium">Network</span>
        </div>
        <div className="grid grid-cols-2 gap-4 text-sm">
          <div>
            <span className="text-text-muted">Sent: </span>
            <span className="font-mono-numbers">{(data.network.bytes_sent / 1024 / 1024).toFixed(2)} MB</span>
          </div>
          <div>
            <span className="text-text-muted">Received: </span>
            <span className="font-mono-numbers">{(data.network.bytes_received / 1024 / 1024).toFixed(2)} MB</span>
          </div>
          <div>
            <span className="text-text-muted">Packets Sent: </span>
            <span className="font-mono-numbers">{data.network.packets_sent.toLocaleString()}</span>
          </div>
          <div>
            <span className="text-text-muted">Packets Received: </span>
            <span className="font-mono-numbers">{data.network.packets_received.toLocaleString()}</span>
          </div>
          <div className="col-span-2">
            <span className="text-text-muted">Error Rate: </span>
            <span className={data.network.error_rate < 0.01 ? 'text-profit' : data.network.error_rate < 0.1 ? 'text-spear' : 'text-loss'}>
              {(data.network.error_rate * 100).toFixed(2)}%
            </span>
          </div>
        </div>
      </div>
    </div>
  )
}
