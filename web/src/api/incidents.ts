import { useQuery } from '@tanstack/react-query'
import type { Incident, ConfigAudit } from '../types'

interface DeadLetterResponse {
  items: Incident[]
  total: number
}

export function useDeadLetterQueue() {
  return useQuery({
    queryKey: ['dead-letter-queue'],
    queryFn: async () => {
      // This endpoint would need to be added to the Operator
      // For now, return mock data
      const mockData: DeadLetterResponse = {
        items: [],
        total: 0,
      }
      return mockData
    },
    refetchInterval: 30000, // Poll every 30 seconds
  })
}

interface ConfigAuditResponse {
  items: ConfigAudit[]
  total: number
}

export function useConfigAudit(limit: number = 50) {
  return useQuery({
    queryKey: ['config-audit', limit],
    queryFn: async () => {
      // This endpoint would need to be added to the Operator
      // For now, return mock data
      const mockData: ConfigAuditResponse = {
        items: [
          {
            id: 1,
            key: 'circuit_breakers.max_loss_24h',
            old_value: '400',
            new_value: '500',
            changed_by: 'admin',
            change_reason: 'Increased loss threshold',
            changed_at: new Date().toISOString(),
          },
          {
            id: 2,
            key: 'wallet:7xKXtg...gAsU',
            old_value: 'CANDIDATE',
            new_value: 'ACTIVE',
            changed_by: 'operator',
            change_reason: 'Promoted via dashboard',
            changed_at: new Date(Date.now() - 3600000).toISOString(),
          },
        ],
        total: 2,
      }
      return mockData
    },
  })
}
