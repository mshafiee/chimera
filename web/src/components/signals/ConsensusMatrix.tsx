import { useMemo } from 'react'
import { Badge } from '../ui/Badge'
import type { ConsensusResponse } from '../../api/consensus'

interface ConsensusMatrixProps {
  data: ConsensusResponse
}

function getConsensusColor(score: number): string {
  if (score >= 0.7) return '#22c55e' // green
  if (score >= 0.5) return '#eab308' // yellow
  return '#ef4444' // red
}

function getConsensusBackgroundColor(score: number): string {
  if (score >= 0.7) return 'rgba(34, 197, 94, 0.1)'
  if (score >= 0.5) return 'rgba(234, 179, 8, 0.1)'
  return 'rgba(239, 68, 68, 0.1)'
}

function getConsensusVariant(score: number): 'success' | 'warning' | 'danger' {
  if (score >= 0.7) return 'success'
  if (score >= 0.5) return 'warning'
  return 'danger'
}

export function ConsensusMatrix({ data }: ConsensusMatrixProps) {
  const matrixData = useMemo(() => {
    if (!data.active_clusters) return []

    return data.active_clusters.flatMap(cluster =>
      cluster.wallets.map(wallet => ({
        walletAddress: wallet,
        consensusScore: cluster.coherence,
        signalCount: cluster.signal_count,
        clusterId: cluster.id,
        avgWQS: cluster.avg_wqs
      }))
    )
  }, [data.active_clusters])

  if (!data || matrixData.length === 0) {
    return (
      <div className="h-64 flex items-center justify-center">
        <div className="text-center text-text-muted">
          <p className="text-sm">No consensus data available</p>
          <p className="text-xs mt-1">Matrix will populate when wallet clusters form</p>
        </div>
      </div>
    )
  }

  return (
    <div className="space-y-4">
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-3">
        {matrixData.map((cell, index) => (
          <div
            key={`${cell.walletAddress}-${index}`}
            className="bg-surface-light rounded-lg p-4 border-2 transition-colors hover:shadow-lg"
            style={{
              borderColor: getConsensusColor(cell.consensusScore),
              backgroundColor: getConsensusBackgroundColor(cell.consensusScore)
            }}
          >
            <div className="flex justify-between items-start mb-2">
              <div className="font-mono text-xs text-text-muted">
                {cell.walletAddress.slice(0, 8)}...{cell.walletAddress.slice(-4)}
              </div>
              <Badge variant={getConsensusVariant(cell.consensusScore)} size="sm">
                {(cell.consensusScore * 100).toFixed(0)}%
              </Badge>
            </div>
            <div className="flex justify-between text-sm">
              <span>{cell.signalCount} signals</span>
              <span className="text-text-muted">WQS: {cell.avgWQS.toFixed(1)}</span>
            </div>
          </div>
        ))}
      </div>
    </div>
  )
}
