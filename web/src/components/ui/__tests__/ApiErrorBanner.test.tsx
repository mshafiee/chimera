import { render, screen } from '@testing-library/react'
import { describe, it, expect } from 'vitest'
import { ApiErrorBanner } from '../ApiErrorBanner'

describe('ApiErrorBanner', () => {
  it('renders nothing when no errors', () => {
    const { container } = render(<ApiErrorBanner errors={[null, undefined]} />)
    expect(container.firstChild).toBeNull()
  })

  it('renders warning when errors present', () => {
    render(<ApiErrorBanner errors={[new Error('API failure')]} />)
    expect(screen.getByText(/API unavailable/i)).toBeInTheDocument()
  })

  it('renders warning with multiple errors', () => {
    render(<ApiErrorBanner errors={[null, new Error('fail'), undefined]} />)
    expect(screen.getByText(/stale/i)).toBeInTheDocument()
  })
})
