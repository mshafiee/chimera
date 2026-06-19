import { MetricCard } from '../ui/MetricCard'
import type { ResourceUsageResponse } from '../../api'
import { Cpu, HardDrive, Wifi, Database, TrendingUp, TrendingDown } from 'lucide-react'

interface ResourceUsageCardProps {
  data: ResourceUsageResponse
}

const getStatusColor = (status: string) => {
  switch (status) {
    case 'normal': return 'text-profit'
    case 'warning': return 'text-spear'
    case 'critical': return 'text-loss'
    default: return 'text-text-muted'
  }
}

const getStatusIcon = (percentage: number) => {
  if (percentage < 50) return TrendingUp
  if (percentage < 80) return null
  return TrendingDown
}

export function ResourceUsageCard({ data }: ResourceUsageCardProps) {
  const formatBytes = (bytes: number) => {
    if (bytes === 0) return '0 MB'
    const mb = bytes / 1024 / 1024
    if (mb < 1024) return `${mb.toFixed(2)} MB`
    const gb = mb / 1024
    return `${gb.toFixed(2)} GB`
  }

  return (
    <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
      {/* CPU */}
      <div className="bg-gradient-to-br from-surface-light to-surface rounded-lg p-4 hover:shadow-md transition-shadow duration-200">
        <div className="flex items-start justify-between mb-3">
          <div className="flex items-center gap-2">
            <Cpu className="w-5 h-5 text-profit" />
            <span className="text-sm font-medium">CPU</span>
          </div>
          {data.cpu.percentage < 50 && <TrendingUp className="w-4 h-4 text-profit" />}
          {data.cpu.percentage >= 80 && <TrendingDown className="w-4 h-4 text-loss" />}
        </div>
        <div className="text-3xl font-bold font-mono-numbers mb-1">
          {data.cpu.percentage.toFixed(0)}%
        </div>
        <div className="flex items-center justify-between text-xs">
          <span className={`font-semibold ${getStatusColor(data.cpu.status)}`}>
            {data.cpu.status.toUpperCase()}
          </span>
          <span className="text-text-muted">
            {data.cpu.current} / {data.cpu.max}
          </span>
        </div>
        {/* Progress bar */}
        <div className="mt-2 h-1.5 bg-surface rounded-full overflow-hidden">
          <div
            className={`h-full rounded-full transition-all duration-500 ${
              data.cpu.status === 'normal' ? 'bg-profit' :
              data.cpu.status === 'warning' ? 'bg-spear' : 'bg-loss'
            }`}
            style={{ width: `${Math.min(data.cpu.percentage, 100)}%` }}
          />
        </div>
      </div>

      {/* Memory */}
      <div className="bg-gradient-to-br from-surface-light to-surface rounded-lg p-4 hover:shadow-md transition-shadow duration-200">
        <div className="flex items-start justify-between mb-3">
          <div className="flex items-center gap-2">
            <Database className="w-5 h-5 text-spear" />
            <span className="text-sm font-medium">Memory</span>
          </div>
          {data.memory.percentage < 50 && <TrendingUp className="w-4 h-4 text-profit" />}
          {data.memory.percentage >= 80 && <TrendingDown className="w-4 h-4 text-loss" />}
        </div>
        <div className="text-3xl font-bold font-mono-numbers mb-1">
          {data.memory.percentage.toFixed(0)}%
        </div>
        <div className="flex items-center justify-between text-xs">
          <span className={`font-semibold ${getStatusColor(data.memory.status)}`}>
            {data.memory.status.toUpperCase()}
          </span>
          <span className="text-text-muted">
            {formatBytes(data.memory.current)}
          </span>
        </div>
        {/* Progress bar */}
        <div className="mt-2 h-1.5 bg-surface rounded-full overflow-hidden">
          <div
            className={`h-full rounded-full transition-all duration-500 ${
              data.memory.status === 'normal' ? 'bg-profit' :
              data.memory.status === 'warning' ? 'bg-spear' : 'bg-loss'
            }`}
            style={{ width: `${Math.min(data.memory.percentage, 100)}%` }}
          />
        </div>
      </div>

      {/* Disk */}
      <div className="bg-gradient-to-br from-surface-light to-surface rounded-lg p-4 hover:shadow-md transition-shadow duration-200">
        <div className="flex items-start justify-between mb-3">
          <div className="flex items-center gap-2">
            <HardDrive className="w-5 h-5 text-shield" />
            <span className="text-sm font-medium">Disk</span>
          </div>
          {data.disk.percentage < 50 && <TrendingUp className="w-4 h-4 text-profit" />}
          {data.disk.percentage >= 80 && <TrendingDown className="w-4 h-4 text-loss" />}
        </div>
        <div className="text-3xl font-bold font-mono-numbers mb-1">
          {data.disk.percentage.toFixed(0)}%
        </div>
        <div className="flex items-center justify-between text-xs">
          <span className={`font-semibold ${getStatusColor(data.disk.status)}`}>
            {data.disk.status.toUpperCase()}
          </span>
          <span className="text-text-muted">
            {formatBytes(data.disk.current)}
          </span>
        </div>
        {/* Progress bar */}
        <div className="mt-2 h-1.5 bg-surface rounded-full overflow-hidden">
          <div
            className={`h-full rounded-full transition-all duration-500 ${
              data.disk.status === 'normal' ? 'bg-profit' :
              data.disk.status === 'warning' ? 'bg-spear' : 'bg-loss'
            }`}
            style={{ width: `${Math.min(data.disk.percentage, 100)}%` }}
          />
        </div>
      </div>

      {/* Network */}
      <div className="bg-gradient-to-br from-surface-light to-surface rounded-lg p-4 hover:shadow-md transition-shadow duration-200">
        <div className="flex items-center gap-2 mb-3">
          <Wifi className="w-5 h-5 text-info" />
          <span className="text-sm font-medium">Network</span>
        </div>
        <div className="space-y-2">
          <div className="flex justify-between items-center">
            <span className="text-xs text-text-muted">Sent</span>
            <span className="text-sm font-mono-numbers font-medium">
              {formatBytes(data.network.bytes_sent)}
            </span>
          </div>
          <div className="flex justify-between items-center">
            <span className="text-xs text-text-muted">Received</span>
            <span className="text-sm font-mono-numbers font-medium">
              {formatBytes(data.network.bytes_received)}
            </span>
          </div>
          <div className="flex justify-between items-center">
            <span className="text-xs text-text-muted">Error Rate</span>
            <span className={`text-sm font-mono-numbers font-medium ${
              data.network.error_rate < 0.01 ? 'text-profit' :
              data.network.error_rate < 0.1 ? 'text-spear' : 'text-loss'
            }`}>
              {(data.network.error_rate * 100).toFixed(2)}%
            </span>
          </div>
        </div>
      </div>
    </div>
  )
}
