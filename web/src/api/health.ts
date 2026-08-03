import { useQuery } from '@tanstack/react-query'
import { apiClient } from './client'
import type { HealthResponse } from '../types'

const HEALTH_ENDPOINT = '/health'
const HEALTH_POLL_INTERVAL_MS = 5000

export function useHealth() {
  return useQuery({
    queryKey: ['health'],
    queryFn: async ({ signal }) => {
      const { data } = await apiClient.get<HealthResponse>(HEALTH_ENDPOINT, { signal })
      return data
    },
    refetchInterval: HEALTH_POLL_INTERVAL_MS,
    retry: 1,
  })
}
