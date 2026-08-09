import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen } from '@testing-library/react'
import { Operations } from '../Operations'

const apiMock = vi.hoisted(() => ({
  useResourceUsage: vi.fn(),
  useSecretRotation: vi.fn(),
  useRateLimitStatus: vi.fn(),
  useHealthCheckDetails: vi.fn(),
}))

vi.mock('../../api', () => apiMock)

const resources = {
  memory: { current: 1, max: 2, percentage: 50, status: 'normal' },
  disk: { current: 1, max: 2, percentage: 40, status: 'normal' },
  cpu: { current: 1, max: 2, percentage: 60, status: 'normal' },
  network: { bytes_sent: 1, bytes_received: 2, packets_sent: 1, packets_received: 2, error_rate: 0.001 },
  timestamp: '2025-01-01T00:00:00Z',
}

function setup(overrides: Record<string, unknown> = {}) {
  apiMock.useResourceUsage.mockReturnValue({
    data: overrides.resources ?? resources,
    isLoading: false,
  })
  apiMock.useSecretRotation.mockReturnValue({
    data: {
      last_rotation_at: null,
      next_rotation_at: null,
      days_until_due: null,
      status: 'never_rotated',
      is_initialized: false,
      rotation_history: [],
    },
    isLoading: false,
  })
  apiMock.useRateLimitStatus.mockReturnValue({
    data: { endpoints: [], overall_status: 'healthy' },
    isLoading: false,
  })
  apiMock.useHealthCheckDetails.mockReturnValue({
    data: {
      overall_status: 'healthy',
      checks: [
        { name: 'db', status: 'passing', message: null, last_check: '2025-01-01T00:00:00Z', response_time_ms: 5 },
        { name: 'rpc', status: 'warning', message: 'slow', last_check: '2025-01-01T00:00:00Z', response_time_ms: 50 },
        { name: 'jito', status: 'failing', message: null, last_check: '2025-01-01T00:00:00Z', response_time_ms: 100 },
      ],
    },
    isLoading: false,
  })
}

beforeEach(() => {
  vi.clearAllMocks()
  setup()
})

describe('Operations', () => {
  it('renders the full operations page with critical system status', () => {
    render(<Operations />)
    expect(screen.getByText('Operations')).toBeInTheDocument()
    expect(screen.getByText('Critical')).toBeInTheDocument()
    expect(screen.getByText('Resource Usage')).toBeInTheDocument()
    expect(screen.getByText('Health Checks')).toBeInTheDocument()
    expect(screen.getByText('Rate Limits')).toBeInTheDocument()
    expect(screen.getByText('Secret Rotation')).toBeInTheDocument()
    expect(screen.getByText('Never Rotated')).toBeInTheDocument()
  })

  it('renders healthy and degraded system statuses', () => {
    apiMock.useHealthCheckDetails.mockReturnValue({
      data: { overall_status: 'healthy', checks: [{ name: 'db', status: 'passing', message: null, last_check: '2025-01-01T00:00:00Z', response_time_ms: 5 }] },
      isLoading: false,
    })
    render(<Operations />)
    expect(screen.getAllByText('Healthy').length).toBeGreaterThan(0)

    apiMock.useHealthCheckDetails.mockReturnValue({
      data: { overall_status: 'degraded', checks: [{ name: 'db', status: 'warning', message: null, last_check: '2025-01-01T00:00:00Z', response_time_ms: 5 }] },
      isLoading: false,
    })
    render(<Operations />)
    expect(screen.getAllByText('Degraded').length).toBeGreaterThan(0)
  })

  it('renders the loading state', () => {
    apiMock.useResourceUsage.mockReturnValue({ data: undefined, isLoading: true })
    apiMock.useHealthCheckDetails.mockReturnValue({ data: undefined, isLoading: true })
    apiMock.useRateLimitStatus.mockReturnValue({ data: undefined, isLoading: true })
    apiMock.useSecretRotation.mockReturnValue({ data: undefined, isLoading: true })
    render(<Operations />)
    expect(screen.getByText('Loading')).toBeInTheDocument()
    expect(screen.getByText('Loading resource data...')).toBeInTheDocument()
    expect(screen.getByText('Loading health checks...')).toBeInTheDocument()
    expect(screen.getByText('Loading rate limit data...')).toBeInTheDocument()
    expect(screen.getByText('Loading rotation data...')).toBeInTheDocument()
  })

  it('renders empty states', () => {
    apiMock.useResourceUsage.mockReturnValue({ data: undefined, isLoading: false })
    apiMock.useHealthCheckDetails.mockReturnValue({ data: undefined, isLoading: false })
    apiMock.useRateLimitStatus.mockReturnValue({ data: undefined, isLoading: false })
    apiMock.useSecretRotation.mockReturnValue({ data: undefined, isLoading: false })
    render(<Operations />)
    expect(screen.getByText('No resource data available')).toBeInTheDocument()
    expect(screen.getByText('No health check data available')).toBeInTheDocument()
    expect(screen.getByText('No rate limit data available')).toBeInTheDocument()
    expect(screen.getByText('No rotation data available')).toBeInTheDocument()
  })
})
