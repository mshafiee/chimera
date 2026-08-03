import { useQuery } from '@tanstack/react-query'
import { apiClient } from './client'
import type { Incident, ConfigAudit } from '../types'

interface Paginated<T> {
  items: T[]
  total: number
}

type DeadLetterResponse = Paginated<Incident>
type ConfigAuditResponse = Paginated<ConfigAudit>

const DEAD_LETTER_ENDPOINT = '/incidents/dead-letter'
const CONFIG_AUDIT_ENDPOINT = '/incidents/config-audit'

export function useDeadLetterQueue() {
  return useQuery({
    queryKey: ['dead-letter-queue'],
    queryFn: async ({ signal }) => {
      const { data } = await apiClient.get<DeadLetterResponse>(DEAD_LETTER_ENDPOINT, { signal })
      return data
    },
    refetchInterval: 30000, // Poll every 30 seconds
  })
}

export function useConfigAudit(params?: { limit?: number; offset?: number }) {
  const limit = params?.limit ?? 50
  const offset = params?.offset ?? 0

  return useQuery({
    queryKey: ['config-audit', limit, offset],
    queryFn: async ({ signal }) => {
      const searchParams = new URLSearchParams()
      if (limit !== undefined) searchParams.set('limit', limit.toString())
      if (offset !== undefined) searchParams.set('offset', offset.toString())

      const { data } = await apiClient.get<ConfigAuditResponse>(
        `${CONFIG_AUDIT_ENDPOINT}?${searchParams.toString()}`,
        { signal }
      )
      return data
    },
  })
}

interface RetryResponse {
  success: boolean
  message: string
  trade_uuid: string
  retry_attempt: number
}

export async function retryDeadLetterItem(tradeUuid: string): Promise<RetryResponse> {
  try {
    const { data } = await apiClient.post<RetryResponse>(
      `/incidents/dead-letter/${encodeURIComponent(tradeUuid)}/retry`
    )
    return data
  } catch (error) {
    throw new Error(
      error instanceof Error ? `Retry failed: ${error.message}` : 'Retry failed. Please try again.'
    )
  }
}
