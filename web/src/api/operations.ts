import { useQuery } from '@tanstack/react-query'
import { toast } from 'sonner'
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

// Fetch Resource Usage
export function useResourceUsage(refetchInterval?: number) {
  return useQuery({
    queryKey: ['operations', 'resources'],
    queryFn: async () => {
      const response = await apiClient.get<ResourceUsageResponse>('/operations/resources')
      return response.data
    },
    refetchInterval,
    staleTime: 5000,
    retry: 3,
    meta: {
      onError: (error: unknown) => {
        console.error('[Operations API] Failed to fetch resource usage:', error)
        // Resource usage is critical - show toast notification
        toast.error('Failed to load resource usage. Please try again later.')
      },
    },
  })
}

// Fetch Secret Rotation Status
export function useSecretRotation() {
  return useQuery({
    queryKey: ['operations', 'secrets'],
    queryFn: async () => {
      const response = await apiClient.get<SecretRotationResponse>('/operations/secrets')
      return response.data
    },
    refetchInterval: 300000, // 5 minutes
    staleTime: 60000,
    retry: 1,
    meta: {
      onError: (error: unknown) => {
        console.error('[Operations API] Failed to fetch secret rotation status:', error)
        // Secret rotation is optional - console only
      },
    },
  })
}

// Fetch Rate Limit Status
export function useRateLimitStatus() {
  return useQuery({
    queryKey: ['operations', 'rate-limit'],
    queryFn: async () => {
      const response = await apiClient.get<RateLimitStatusResponse>('/operations/rate-limit')
      return response.data
    },
    refetchInterval: 10000,
    staleTime: 5000,
    retry: 1,
    meta: {
      onError: (error: unknown) => {
        console.error('[Operations API] Failed to fetch rate limit status:', error)
        // Rate limit status is optional - console only
      },
    },
  })
}

// Fetch System Logs
export function useSystemLogs(level?: string, limit?: number) {
  return useQuery({
    queryKey: ['operations', 'logs', level, limit],
    queryFn: async () => {
      const response = await apiClient.get<SystemLogsResponse>('/operations/logs', {
        params: {
          ...(level && { level }),
          ...(limit && { limit }),
        },
      })
      return response.data
    },
    refetchInterval: 30000,
    staleTime: 10000,
    retry: 1,
    meta: {
      onError: (error: unknown) => {
        console.error('[Operations API] Failed to fetch system logs:', error)
        // System logs are optional - console only
      },
    },
  })
}

// Fetch Health Check Details
export function useHealthCheckDetails() {
  return useQuery({
    queryKey: ['operations', 'health-checks'],
    queryFn: async () => {
      const response = await apiClient.get<HealthCheckDetailsResponse>('/operations/health-checks')
      return response.data
    },
    refetchInterval: 30000,
    staleTime: 10000,
    retry: 3,
    meta: {
      onError: (error: unknown) => {
        console.error('[Operations API] Failed to fetch health check details:', error)
        // Health checks are important - show toast notification
        toast.error('Failed to load health check details. Please try again later.')
      },
    },
  })
}
