import { describe, it, expect } from 'vitest'
import { render, screen } from '@testing-library/react'
import { HealthChecksCard } from '../HealthChecksCard'
import { RateLimitStatusCard } from '../RateLimitStatusCard'
import { ResourceUsageCard } from '../ResourceUsageCard'
import { SecretRotationCard } from '../SecretRotationCard'
import * as operationsBarrel from '../index'

const healthData = {
  overall_status: 'degraded' as const,
  checks: [
    { name: 'db', status: 'passing' as const, message: null, last_check: '2025-01-01T00:00:00Z', response_time_ms: 5 },
    { name: 'rpc', status: 'warning' as const, message: 'slow rpc', last_check: '2025-01-01T00:00:00Z', response_time_ms: 100 },
    { name: 'jito', status: 'failing' as const, message: 'jito down', last_check: '2025-01-01T00:00:00Z', response_time_ms: 200 },
  ],
}

const rateLimitData = {
  overall_status: 'throttled' as const,
  endpoints: [
    { endpoint: '/api/v1/trades', current_rate: 50, limit: 60, window_seconds: 60, remaining: 10, reset_at: '2025-01-01T00:00:00Z', utilization_percent: 83, status: 'warning' as const },
    { endpoint: '/api/v1/wallets', current_rate: 10, limit: 100, window_seconds: 60, remaining: 90, reset_at: '2025-01-01T00:00:00Z', utilization_percent: 10, status: 'ok' as const },
    { endpoint: '/api/v1/health', current_rate: 100, limit: 50, window_seconds: 60, remaining: 0, reset_at: '2025-01-01T00:00:00Z', utilization_percent: 200, status: 'throttled' as const },
  ],
}

const resourcesData = {
  cpu: { current: 4, max: 8, percentage: 50, status: 'normal' as const },
  memory: { current: 2 * 1024 * 1024 * 1024, max: 8 * 1024 * 1024 * 1024, percentage: 85, status: 'critical' as const },
  disk: { current: 512 * 1024 * 1024, max: 1024 * 1024 * 1024, percentage: 40, status: 'warning' as const },
  network: { bytes_sent: 1024, bytes_received: 2048, packets_sent: 1, packets_received: 2, error_rate: 0.001 },
  timestamp: '2025-01-01T00:00:00Z',
}

const secretData = {
  last_rotation_at: '2025-01-01T00:00:00Z',
  next_rotation_at: '2025-02-01T00:00:00Z',
  days_until_due: -3,
  status: 'overdue' as const,
  is_initialized: true,
  rotation_history: [
    { timestamp: '2025-01-01T00:00:00Z', status: 'success' as const, duration_seconds: 12.5, keys_rotated: 2, failed_keys: 0 },
    { timestamp: '2024-12-01T00:00:00Z', status: 'failed' as const, duration_seconds: null, keys_rotated: 0, failed_keys: 2 },
    { timestamp: '2024-11-01T00:00:00Z', status: 'partial' as const, duration_seconds: 5, keys_rotated: 1, failed_keys: 1 },
  ],
}

describe('operations barrel', () => {
  it('re-exports all components', () => {
    expect(operationsBarrel.ResourceUsageCard).toBeTruthy()
    expect(operationsBarrel.SecretRotationCard).toBeTruthy()
    expect(operationsBarrel.RateLimitStatusCard).toBeTruthy()
    expect(operationsBarrel.HealthChecksCard).toBeTruthy()
  })
})

describe('HealthChecksCard', () => {
  it('renders overall status, counts and checks', () => {
    render(<HealthChecksCard data={healthData} />)
    expect(screen.getByText('Degraded')).toBeInTheDocument()
    expect(screen.getByText('passing')).toBeInTheDocument()
    expect(screen.getByText('slow rpc')).toBeInTheDocument()
    expect(screen.getByText('jito down')).toBeInTheDocument()
  })

  it('renders a healthy overall status', () => {
    render(<HealthChecksCard data={{ overall_status: 'healthy', checks: [] }} />)
    expect(screen.getByText('Healthy')).toBeInTheDocument()
  })

  it('handles unknown check statuses by falling back to failing', () => {
    render(
      <HealthChecksCard
        data={{
          overall_status: 'unhealthy',
          checks: [{ name: 'x', status: 'weird' as never, message: null, last_check: '2025-01-01T00:00:00Z', response_time_ms: 1 }],
        }}
      />
    )
    expect(screen.getByText('Failing')).toBeInTheDocument()
  })
})

describe('RateLimitStatusCard', () => {
  it('renders endpoints and quick stats', () => {
    render(<RateLimitStatusCard data={rateLimitData} />)
    expect(screen.getByText('/api/v1/trades')).toBeInTheDocument()
    expect(screen.getByText('83%')).toBeInTheDocument()
    expect(screen.getByText('10 remaining')).toBeInTheDocument()
  })

  it('renders healthy and degraded overall statuses', () => {
    render(
      <RateLimitStatusCard
        data={{ overall_status: 'healthy', endpoints: [{ endpoint: 'a', current_rate: 1, limit: 10, window_seconds: 60, remaining: 9, reset_at: 'x', utilization_percent: 5, status: 'ok' }] }}
      />
    )
    expect(screen.getByText('healthy')).toBeInTheDocument()
    render(
      <RateLimitStatusCard
        data={{ overall_status: 'degraded', endpoints: [] }}
      />
    )
  })

  it('renders unknown endpoint statuses', () => {
    render(
      <RateLimitStatusCard
        data={{ overall_status: 'healthy', endpoints: [{ endpoint: 'b', current_rate: 1, limit: 10, window_seconds: 60, remaining: 9, reset_at: 'x', utilization_percent: 5, status: 'mystery' as never }] }}
      />
    )
    expect(screen.getByText('mystery')).toBeInTheDocument()
  })
})

describe('ResourceUsageCard', () => {
  it('renders all resource cards with byte formatting', () => {
    const { container } = render(<ResourceUsageCard data={resourcesData} />)
    expect(container.textContent).toContain('CPU')
    expect(container.textContent).toContain('Memory')
    expect(container.textContent).toContain('Disk')
    expect(container.textContent).toContain('Network')
    expect(container.textContent).toContain('2.00 GB')
    expect(container.textContent).toContain('512.00 MB')
  })

  it('renders warning cpu status', () => {
    const { container } = render(
      <ResourceUsageCard
        data={{
          cpu: { current: 8, max: 8, percentage: 95, status: 'warning' },
          memory: { current: 1, max: 2, percentage: 60, status: 'normal' },
          disk: { current: 1, max: 2, percentage: 60, status: 'normal' },
          network: { bytes_sent: 0, bytes_received: 0, packets_sent: 0, packets_received: 0, error_rate: 0.05 },
          timestamp: 'x',
        }}
      />
    )
    expect(container.textContent).toContain('WARNING')
  })

  it('formats zero bytes and healthy states', () => {
    const { container } = render(
      <ResourceUsageCard
        data={{
          cpu: { current: 0, max: 1, percentage: 30, status: 'normal' },
          memory: { current: 0, max: 1, percentage: 30, status: 'normal' },
          disk: { current: 0, max: 1, percentage: 30, status: 'normal' },
          network: { bytes_sent: 0, bytes_received: 0, packets_sent: 0, packets_received: 0, error_rate: 0.2 },
          timestamp: 'x',
        }}
      />
    )
    expect(container.textContent).toContain('0 MB')
  })
})

describe('SecretRotationCard', () => {
  it('renders overdue status with history table', () => {
    render(<SecretRotationCard data={secretData} />)
    expect(screen.getByText('Overdue')).toBeInTheDocument()
    expect(screen.getByText('3 days overdue')).toBeInTheDocument()
    expect(screen.getByText('3')).toBeInTheDocument() // abs days
    expect(screen.getByText('12.5s')).toBeInTheDocument()
  })

  it('renders active status with positive days remaining', () => {
    render(
      <SecretRotationCard
        data={{ ...secretData, status: 'active', days_until_due: 15, rotation_history: [] }}
      />
    )
    expect(screen.getByText('Active')).toBeInTheDocument()
    expect(screen.getByText('15 days remaining')).toBeInTheDocument()
  })

  it('renders due_soon and never_rotated guidance', () => {
    render(
      <SecretRotationCard
        data={{ ...secretData, status: 'due_soon', days_until_due: 5, rotation_history: [] }}
      />
    )
    expect(screen.getByText('Due Soon')).toBeInTheDocument()

    render(
      <SecretRotationCard
        data={{ ...secretData, status: 'never_rotated', last_rotation_at: null, next_rotation_at: null, days_until_due: null, rotation_history: [] }}
      />
    )
    expect(screen.getByText('Never Rotated')).toBeInTheDocument()
    expect(screen.getByText(/Fresh Deployment/)).toBeInTheDocument()
    expect(screen.getByText('Never')).toBeInTheDocument()
    expect(screen.getByText('Not scheduled')).toBeInTheDocument()
  })

  it('renders unknown status without rotation dates', () => {
    render(
      <SecretRotationCard
        data={{ ...secretData, status: 'unknown' as never, last_rotation_at: null, next_rotation_at: null, days_until_due: null, rotation_history: [] }}
      />
    )
    expect(screen.getByText('Unknown')).toBeInTheDocument()
  })
})
