import { describe, it, expect, vi, afterEach } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { ErrorBoundary } from '../ErrorBoundary'

function Bomb(): never {
  throw new Error('KABOOM')
}

describe('ErrorBoundary', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('renders children when there is no error', () => {
    render(
      <ErrorBoundary>
        <div>Safe content</div>
      </ErrorBoundary>
    )
    expect(screen.getByText('Safe content')).toBeInTheDocument()
  })

  it('renders the default fallback when a child throws', () => {
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(
      <ErrorBoundary>
        <Bomb />
      </ErrorBoundary>
    )
    expect(screen.getByText('Something went wrong')).toBeInTheDocument()
    expect(screen.getByText('KABOOM')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /reload/i })).toBeInTheDocument()
    errorSpy.mockRestore()
  })

  it('renders a custom fallback when provided', () => {
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(
      <ErrorBoundary fallback={<div>Custom fallback</div>}>
        <Bomb />
      </ErrorBoundary>
    )
    expect(screen.getByText('Custom fallback')).toBeInTheDocument()
    expect(screen.queryByText('Something went wrong')).not.toBeInTheDocument()
    errorSpy.mockRestore()
  })

  it('reloads the page when the retry button is clicked', () => {
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    const reloadSpy = vi.fn()
    const originalLocation = window.location
    Object.defineProperty(window, 'location', {
      configurable: true,
      writable: true,
      value: { ...originalLocation, reload: reloadSpy },
    })
    render(
      <ErrorBoundary>
        <Bomb />
      </ErrorBoundary>
    )
    fireEvent.click(screen.getByRole('button', { name: /reload/i }))
    expect(reloadSpy).toHaveBeenCalled()
    errorSpy.mockRestore()
  })
})
