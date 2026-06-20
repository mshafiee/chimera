import { useQuery } from '@tanstack/react-query'
import { apiClient } from './client'

// Resource Usage Response
export interface ResourceUsageResponse {
  memory: ResourceMetric
  disk: ResourceMetric
  cpu: ResourceMetric
  network: NetworkMetric
  timestamp: string
}

export interface ResourceMetric {
  current: number
  max: number
  percentage: number
  status: 'normal' | 'warning' | 'critical'
}

export interface NetworkMetric {
  bytes_sent: number
  bytes_received: number
  packets_sent: number
  packets_received: number
  error_rate: number
}

// Secret Rotation Response
export interface SecretRotationResponse {
  last_rotation_at: string | null
  next_rotation_at: string | null
  days_until_due: number | null
  status: 'active' | 'due_soon' | 'overdue' | 'never_rotated' | 'unknown'
  is_initialized: boolean  // true if rotation tracking is configured
  rotation_history: RotationEvent[]
}

export interface RotationEvent {
  timestamp: string
  status: 'success' | 'failed' | 'partial'
  duration_seconds: number | null
  keys_rotated: number
  failed_keys: number
}

// Rate Limit Status Response
export interface RateLimitStatusResponse {
  endpoints: RateLimitEndpoint[]
  overall_status: 'healthy' | 'degraded' | 'throttled'
}

export interface RateLimitEndpoint {
  endpoint: string
  current_rate: number
  limit: number
  window_seconds: number
  remaining: number
  reset_at: string
  utilization_percent: number
  status: 'ok' | 'warning' | 'throttled'
}

// System Logs Response
export interface SystemLogsResponse {
  logs: SystemLog[]
  total_count: number
  log_levels: LogLevelStats[]
}

export interface SystemLog {
  id: number
  timestamp: string
  level: 'debug' | 'info' | 'warn' | 'error'
  component: string
  message: string
  context?: Record<string, unknown>
}

export interface LogLevelStats {
  level: string
  count: number
  percentage: number
}

// Health Check Details
export interface HealthCheckDetailsResponse {
  overall_status: 'healthy' | 'degraded' | 'unhealthy'
  checks: HealthCheck[]
}

export interface HealthCheck {
  name: string
  status: 'passing' | 'warning' | 'failing'
  message: string | null
  last_check: string
  response_time_ms: number
}

// Mock data for when API is not available
const mockResourceUsage: ResourceUsageResponse = {
  memory: { current: 0, max: 0, percentage: 0, status: 'normal' },
  disk: { current: 0, max: 0, percentage: 0, status: 'normal' },
  cpu: { current: 0, max: 0, percentage: 0, status: 'normal' },
  network: { bytes_sent: 0, bytes_received: 0, packets_sent: 0, packets_received: 0, error_rate: 0 },
  timestamp: new Date().toISOString()
}

const mockSecretRotation: SecretRotationResponse = {
  last_rotation_at: null,
  next_rotation_at: null,
  days_until_due: null,
  status: 'unknown',
  is_initialized: false,
  rotation_history: []
}

const mockRateLimitStatus: RateLimitStatusResponse = {
  endpoints: [],
  overall_status: 'healthy'
}

const mockSystemLogs: SystemLogsResponse = {
  logs: [],
  total_count: 0,
  log_levels: []
}

const mockHealthCheckDetails: HealthCheckDetailsResponse = {
  overall_status: 'healthy',
  checks: []
}

// Fetch Resource Usage
export function useResourceUsage(refetchInterval?: number) {
  return useQuery({
    queryKey: ['operations', 'resources'],
    queryFn: async () => {
      try {
        const response = await apiClient.get<ResourceUsageResponse>('/operations/resources')
        return response.data
      } catch (error: any) {
        if (error.response?.status === 404) {
          console.warn('[Operations API] Resources endpoint not implemented, using mock data')
          return mockResourceUsage
        }
        throw error
      }
    },
    refetchInterval,
    staleTime: 5000,
  })
}

// Fetch Secret Rotation Status
export function useSecretRotation() {
  return useQuery({
    queryKey: ['operations', 'secrets'],
    queryFn: async () => {
      try {
        const response = await apiClient.get<SecretRotationResponse>('/operations/secrets')
        return response.data
      } catch (error: any) {
        if (error.response?.status === 404) {
          console.warn('[Operations API] Secrets endpoint not implemented, using mock data')
          return mockSecretRotation
        }
        throw error
      }
    },
    refetchInterval: 300000, // 5 minutes
    staleTime: 60000,
  })
}

// Fetch Rate Limit Status
export function useRateLimitStatus() {
  return useQuery({
    queryKey: ['operations', 'rate-limit'],
    queryFn: async () => {
      try {
        const response = await apiClient.get<RateLimitStatusResponse>('/operations/rate-limit')
        return response.data
      } catch (error: any) {
        if (error.response?.status === 404) {
          console.warn('[Operations API] Rate limit endpoint not implemented, using mock data')
          return mockRateLimitStatus
        }
        throw error
      }
    },
    refetchInterval: 10000,
    staleTime: 5000,
  })
}

// Fetch System Logs
export function useSystemLogs(level?: string, limit?: number) {
  return useQuery({
    queryKey: ['operations', 'logs', level, limit],
    queryFn: async () => {
      try {
        const response = await apiClient.get<SystemLogsResponse>('/operations/logs', {
          params: {
            ...(level && { level }),
            ...(limit && { limit }),
          },
        })
        return response.data
      } catch (error: any) {
        if (error.response?.status === 404) {
          console.warn('[Operations API] Logs endpoint not implemented, using mock data')
          return mockSystemLogs
        }
        throw error
      }
    },
    refetchInterval: 30000,
    staleTime: 10000,
  })
}

// Fetch Health Check Details
export function useHealthCheckDetails() {
  return useQuery({
    queryKey: ['operations', 'health-checks'],
    queryFn: async () => {
      try {
        const response = await apiClient.get<HealthCheckDetailsResponse>('/operations/health-checks')
        return response.data
      } catch (error: any) {
        if (error.response?.status === 404) {
          console.warn('[Operations API] Health checks endpoint not implemented, using mock data')
          return mockHealthCheckDetails
        }
        throw error
      }
    },
    refetchInterval: 30000,
    staleTime: 10000,
  })
}
