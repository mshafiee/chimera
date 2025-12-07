import { useQuery } from '@tanstack/react-query'
import { apiClient } from './client'
import type { Incident, ConfigAudit } from '../types'

interface DeadLetterResponse {
  items: Incident[]
  total: number
}

export function useDeadLetterQueue() {
  return useQuery({
    queryKey: ['dead-letter-queue'],
    queryFn: async () => {
      const { data } = await apiClient.get<DeadLetterResponse>('/incidents/dead-letter')
      return data
    },
    refetchInterval: 30000, // Poll every 30 seconds
  })
}

interface ConfigAuditResponse {
  items: ConfigAudit[]
  total: number
}

export function useConfigAudit(params?: { limit?: number; offset?: number }) {
  const limit = params?.limit ?? 50
  const offset = params?.offset ?? 0
  
  return useQuery({
    queryKey: ['config-audit', limit, offset],
    queryFn: async () => {
      const searchParams = new URLSearchParams()
      if (limit) searchParams.set('limit', limit.toString())
      if (offset) searchParams.set('offset', offset.toString())
      
      const { data } = await apiClient.get<ConfigAuditResponse>(
        `/incidents/config-audit?${searchParams.toString()}`
      )
      return data
    },
  })
}
