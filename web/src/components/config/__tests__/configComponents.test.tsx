import { describe, it, expect, vi } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { ConfigArrayInput } from '../ConfigArrayInput'
import { ConfigInput } from '../ConfigInput'
import { ConfigSection } from '../ConfigSection'
import { ConfigToggle } from '../ConfigToggle'
import * as configBarrel from '../index'

describe('config barrel', () => {
  it('re-exports all components', () => {
    expect(configBarrel.ConfigSection).toBeTruthy()
    expect(configBarrel.ConfigInput).toBeTruthy()
    expect(configBarrel.ConfigToggle).toBeTruthy()
    expect(configBarrel.ConfigArrayInput).toBeTruthy()
  })
})

describe('ConfigArrayInput', () => {
  it('renders values, adds and removes items', () => {
    const onChange = vi.fn()
    const { container } = render(
      <ConfigArrayInput label="Targets" unit="%" values={[25, 50]} onChange={onChange} description="desc" />
    )
    expect(screen.getByText('Targets')).toBeInTheDocument()
    expect(container.querySelectorAll('input')).toHaveLength(2)

    const removeButtons = container.querySelectorAll('button')
    fireEvent.click(removeButtons[0])
    expect(onChange).toHaveBeenCalledWith([50])

    fireEvent.click(screen.getByText('Add Target'))
    expect(onChange).toHaveBeenCalledWith([25, 50, 75])
  })

  it('adds from zero values and edits items', () => {
    const onChange = vi.fn()
    render(<ConfigArrayInput label="T" values={[]} onChange={onChange} />)
    fireEvent.click(screen.getByText('Add Target'))
    expect(onChange).toHaveBeenCalledWith([25])

    render(<ConfigArrayInput label="T" values={[10]} onChange={onChange} />)
    const input = document.querySelector('input') as HTMLInputElement
    fireEvent.change(input, { target: { value: '42' } })
    expect(onChange).toHaveBeenCalledWith([42])
  })

  it('respects min/max constraints and disabled state', () => {
    const onChange = vi.fn()
    render(
      <ConfigArrayInput label="T" values={[10]} onChange={onChange} min={0} max={100} disabled />
    )
    const input = document.querySelector('input') as HTMLInputElement
    fireEvent.change(input, { target: { value: '-5' } })
    fireEvent.change(input, { target: { value: '500' } })
    expect(onChange).not.toHaveBeenCalled()
    expect(screen.queryByText('Add Target')).not.toBeInTheDocument()
  })
})

describe('ConfigInput', () => {
  it('renders text inputs', () => {
    const onChange = vi.fn()
    render(<ConfigInput label="Name" value="abc" onChange={onChange} description="help" />)
    fireEvent.change(document.querySelector('input') as HTMLInputElement, {
      target: { value: 'xyz' },
    })
    expect(onChange).toHaveBeenCalledWith('xyz')
  })

  it('parses number inputs and falls back to 0', () => {
    const onChange = vi.fn()
    render(<ConfigInput label="N" value={1} onChange={onChange} type="number" unit="SOL" />)
    const input = document.querySelector('input') as HTMLInputElement
    fireEvent.change(input, { target: { value: '12.5' } })
    expect(onChange).toHaveBeenCalledWith(12.5)
    fireEvent.change(input, { target: { value: '' } })
    expect(onChange).toHaveBeenCalledWith(0)
    fireEvent.change(input, { target: { value: 'not-a-number' } })
    expect(onChange).toHaveBeenCalledWith(0)
  })

  it('renders errors instead of descriptions', () => {
    render(<ConfigInput label="E" value="" onChange={vi.fn()} error="bad value" description="hidden" />)
    expect(screen.getByText('bad value')).toBeInTheDocument()
    expect(screen.queryByText('hidden')).not.toBeInTheDocument()
  })

  it('renders disabled with unit', () => {
    const { container } = render(
      <ConfigInput label="D" value={5} onChange={vi.fn()} type="number" disabled unit="%" />
    )
    expect(container.querySelector('input')).toBeDisabled()
    expect(container.textContent).toContain('%')
  })
})

describe('ConfigSection', () => {
  it('toggles content when collapsible', () => {
    const { _container } = render(
      <ConfigSection title="Section" description="d" icon={<span>icon</span>} badge={<span>badge</span>}>
        <div>Inner content</div>
      </ConfigSection>
    )
    expect(screen.getByText('Inner content')).toBeInTheDocument()
    expect(screen.getByText('icon')).toBeInTheDocument()
    expect(screen.getByText('badge')).toBeInTheDocument()
    fireEvent.click(screen.getByText('Section'))
    expect(screen.queryByText('Inner content')).not.toBeInTheDocument()
  })

  it('respects defaultOpen=false and non-collapsible', () => {
    const { _container } = render(
      <ConfigSection title="Closed" defaultOpen={false}>
        <div>Hidden</div>
      </ConfigSection>
    )
    expect(screen.queryByText('Hidden')).not.toBeInTheDocument()

    const { container: _c2 } = render(
      <ConfigSection title="Fixed" collapsible={false}>
        <div>Visible</div>
      </ConfigSection>
    )
    expect(screen.getByText('Visible')).toBeInTheDocument()
    fireEvent.click(screen.getByText('Fixed'))
    expect(screen.getByText('Visible')).toBeInTheDocument()
  })
})

describe('ConfigToggle', () => {
  it('toggles on click and respects disabled', () => {
    const onChange = vi.fn()
    render(<ConfigToggle label="Toggle" description="desc" enabled={false} onChange={onChange} />)
    fireEvent.click(screen.getByRole('switch'))
    expect(onChange).toHaveBeenCalledWith(true)

    render(<ConfigToggle label="D" enabled disabled onChange={onChange} />)
    const disabledSwitch = screen.getAllByRole('switch')[1]
    fireEvent.click(disabledSwitch)
    expect(onChange).toHaveBeenCalledTimes(1)
  })

  it('renders badge and enabled state', () => {
    const { _container } = render(
      <ConfigToggle label="On" enabled onChange={vi.fn()} badge={<span>b</span>} />
    )
    expect(screen.getByText('b')).toBeInTheDocument()
    expect(screen.getByRole('switch')).toHaveAttribute('aria-checked', 'true')
  })
})
