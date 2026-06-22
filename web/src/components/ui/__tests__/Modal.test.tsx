import { describe, it, expect, vi } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { Modal } from '../Modal'

describe('Modal', () => {
  it('renders when isOpen is true', () => {
    render(<Modal isOpen={true} onClose={vi.fn()}>Content</Modal>)
    expect(screen.getByText('Content')).toBeInTheDocument()
  })

  it('does not render when isOpen is false', () => {
    render(<Modal isOpen={false} onClose={vi.fn()}>Content</Modal>)
    expect(screen.queryByText('Content')).not.toBeInTheDocument()
  })

  it('renders title when provided', () => {
    render(<Modal isOpen={true} onClose={vi.fn()} title="My Modal">Content</Modal>)
    expect(screen.getByText('My Modal')).toBeInTheDocument()
  })

  it('calls onClose when Escape key is pressed', () => {
    const onClose = vi.fn()
    render(<Modal isOpen={true} onClose={onClose}>Content</Modal>)
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(onClose).toHaveBeenCalledTimes(1)
  })

  it('calls onClose when backdrop is clicked', () => {
    const onClose = vi.fn()
    render(<Modal isOpen={true} onClose={onClose}>Content</Modal>)
    const backdrop = document.querySelector('[aria-hidden="true"]')
    expect(backdrop).not.toBeNull()
    if (backdrop) fireEvent.click(backdrop)
    expect(onClose).toHaveBeenCalledTimes(1)
  })

  it('sets role="dialog" and aria-modal="true"', () => {
    render(<Modal isOpen={true} onClose={vi.fn()} title="Test">Content</Modal>)
    const dialog = screen.getByRole('dialog')
    expect(dialog).toHaveAttribute('aria-modal', 'true')
    expect(dialog).toHaveAttribute('aria-labelledby', 'modal-title')
  })

  it('close button has aria-label', () => {
    render(<Modal isOpen={true} onClose={vi.fn()} title="Test">Content</Modal>)
    expect(screen.getByLabelText('Close dialog')).toBeInTheDocument()
  })

  it('does not close when clicking inside the modal panel', () => {
    const onClose = vi.fn()
    render(<Modal isOpen={true} onClose={onClose}>Content</Modal>)
    const panel = screen.getByRole('dialog')
    fireEvent.click(panel)
    expect(onClose).not.toHaveBeenCalled()
  })
})
