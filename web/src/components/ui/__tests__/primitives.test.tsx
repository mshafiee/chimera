import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { Button } from '../Button'
import { Card, CardHeader, CardTitle, CardContent } from '../Card'
import { Badge, StatusBadge, StrategyBadge } from '../Badge'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '../Table'
import { ConfirmModal } from '../Modal'
import { ToastContainer, toast, useToastStore } from '../Toast'
import { MetricCard } from '../MetricCard'
import { TimeRangePicker, DateRangePicker } from '../TimeRangePicker'
import { RealtimeBadge, ConnectionStatus } from '../RealtimeBadge'
import { LoadingSpinner } from '../LoadingSpinner'
import { ApiErrorBanner } from '../ApiErrorBanner'
import * as uiBarrel from '../index'

describe('ui barrel', () => {
  it('re-exports all primitives', () => {
    expect(uiBarrel.Button).toBeTruthy()
    expect(uiBarrel.Card).toBeTruthy()
    expect(uiBarrel.CardHeader).toBeTruthy()
    expect(uiBarrel.CardTitle).toBeTruthy()
    expect(uiBarrel.CardContent).toBeTruthy()
    expect(uiBarrel.Badge).toBeTruthy()
    expect(uiBarrel.StatusBadge).toBeTruthy()
    expect(uiBarrel.StrategyBadge).toBeTruthy()
    expect(uiBarrel.Table).toBeTruthy()
    expect(uiBarrel.TableHeader).toBeTruthy()
    expect(uiBarrel.TableBody).toBeTruthy()
    expect(uiBarrel.TableRow).toBeTruthy()
    expect(uiBarrel.TableHead).toBeTruthy()
    expect(uiBarrel.TableCell).toBeTruthy()
    expect(uiBarrel.Modal).toBeTruthy()
    expect(uiBarrel.ConfirmModal).toBeTruthy()
    expect(uiBarrel.ToastContainer).toBeTruthy()
    expect(uiBarrel.toast).toBeTruthy()
    expect(uiBarrel.useToastStore).toBeTruthy()
    expect(uiBarrel.MetricCard).toBeTruthy()
    expect(uiBarrel.TimeRangePicker).toBeTruthy()
    expect(uiBarrel.DateRangePicker).toBeTruthy()
    expect(uiBarrel.RealtimeBadge).toBeTruthy()
    expect(uiBarrel.ConnectionStatus).toBeTruthy()
  })
})

describe('Button', () => {
  it('renders children with default styles', () => {
    render(<Button>Click</Button>)
    expect(screen.getByRole('button', { name: 'Click' })).toBeInTheDocument()
  })

  it('applies variant and size classes', () => {
    const { container } = render(
      <Button variant="danger" size="lg">Danger</Button>
    )
    const btn = container.querySelector('button')
    expect(btn?.className).toContain('bg-loss')
    expect(btn?.className).toContain('rounded-lg')
  })

  it('disables when loading and renders the spinner', () => {
    const onClick = vi.fn()
    const { container } = render(
      <Button loading onClick={onClick}>Save</Button>
    )
    const btn = container.querySelector('button')
    expect(btn).toBeDisabled()
    expect(container.querySelector('svg')).not.toBeNull()
  })

  it('fires onClick and respects disabled', () => {
    const onClick = vi.fn()
    const { rerender } = render(<Button onClick={onClick}>Go</Button>)
    fireEvent.click(screen.getByRole('button', { name: 'Go' }))
    expect(onClick).toHaveBeenCalledTimes(1)
    rerender(<Button disabled onClick={onClick}>Go</Button>)
    fireEvent.click(screen.getByRole('button', { name: 'Go' }))
    expect(onClick).toHaveBeenCalledTimes(1)
  })

  it('renders all variants', () => {
    for (const variant of ['primary', 'secondary', 'danger', 'ghost', 'shield', 'spear'] as const) {
      const { container } = render(<Button variant={variant}>{variant}</Button>)
      expect(container.querySelector('button')).not.toBeNull()
    }
  })
})

describe('Badge', () => {
  it('renders with variants and sizes', () => {
    for (const variant of ['default', 'success', 'warning', 'danger', 'shield', 'spear', 'info'] as const) {
      const { container } = render(<Badge variant={variant}>{variant}</Badge>)
      expect(container.querySelector('span')).not.toBeNull()
    }
    const { container } = render(<Badge size="md">Big</Badge>)
    expect(container.querySelector('span')?.className).toContain('text-sm')
  })

  it('renders a default variant when none given', () => {
    const { container } = render(<Badge>Plain</Badge>)
    expect(container.querySelector('span')?.className).toContain('bg-surface-light')
  })

  it('StatusBadge maps known and unknown statuses', () => {
    const known = ['ACTIVE', 'EXITING', 'CLOSED', 'PENDING', 'QUEUED', 'EXECUTING', 'FAILED', 'RETRY', 'DEAD_LETTER', 'CANDIDATE', 'REJECTED']
    for (const status of known) {
      const { container } = render(<StatusBadge status={status} />)
      expect(container.textContent).toContain(status === 'DEAD_LETTER' ? 'Dead Letter' : status.charAt(0) + status.slice(1).toLowerCase())
    }
    render(<StatusBadge status="WEIRD" />)
    expect(screen.getByText('WEIRD')).toBeInTheDocument()
  })

  it('StrategyBadge renders all strategies', () => {
    render(<StrategyBadge strategy="SHIELD" />)
    expect(screen.getByText('🛡️ Shield')).toBeInTheDocument()
    render(<StrategyBadge strategy="SPEAR" />)
    expect(screen.getByText('⚔️ Spear')).toBeInTheDocument()
    render(<StrategyBadge strategy="EXIT" />)
    expect(screen.getByText('Exit')).toBeInTheDocument()
  })
})

describe('Card family', () => {
  it('renders with variants and paddings', () => {
    for (const variant of ['default', 'shield', 'spear'] as const) {
      const { container } = render(<Card variant={variant}>Card</Card>)
      expect(container.querySelector('div')?.textContent).toBe('Card')
    }
    for (const padding of ['none', 'sm', 'md', 'lg'] as const) {
      const { container } = render(<Card padding={padding}>Card</Card>)
      expect(container.querySelector('div')).not.toBeNull()
    }
  })

  it('renders header, title and content', () => {
    render(
      <Card>
        <CardHeader><CardTitle>Title</CardTitle></CardHeader>
        <CardContent>Body</CardContent>
      </Card>
    )
    expect(screen.getByText('Title')).toBeInTheDocument()
    expect(screen.getByText('Body')).toBeInTheDocument()
  })
})

describe('Table primitives', () => {
  it('renders a full table with sortable headers', () => {
    const { container } = render(
      <Table>
        <TableHeader>
          <TableRow hoverable={false}>
            <TableHead sortable sorted="asc">Name</TableHead>
            <TableHead sortable sorted="desc">Age</TableHead>
            <TableHead sortable>Score</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow><TableCell mono>Cell</TableCell></TableRow>
        </TableBody>
      </Table>
    )
    expect(container.querySelector('table')).not.toBeNull()
    expect(screen.getByText('↑')).toBeInTheDocument()
    expect(screen.getByText('↓')).toBeInTheDocument()
    expect(screen.getByText('↕')).toBeInTheDocument()
    expect(screen.getByText('Cell')).toBeInTheDocument()
  })

  it('renders non-hoverable rows', () => {
    const { container } = render(
      <TableRow hoverable={false}><TableCell>x</TableCell></TableRow>
    )
    expect(container.querySelector('tr')?.className).not.toContain('hover:bg-surface-light')
  })
})

describe('Modal and ConfirmModal', () => {
  it('ConfirmModal renders labels and calls callbacks', () => {
    const onClose = vi.fn()
    const onConfirm = vi.fn()
    render(
      <ConfirmModal
        isOpen
        onClose={onClose}
        onConfirm={onConfirm}
        title="Confirm"
        message="Are you sure?"
        confirmLabel="Yes"
        cancelLabel="No"
        variant="danger"
      />
    )
    expect(screen.getByText('Are you sure?')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Yes' }))
    expect(onConfirm).toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'No' }))
    expect(onClose).toHaveBeenCalled()
  })

  it('ConfirmModal disables cancel while loading', () => {
    render(
      <ConfirmModal
        isOpen
        onClose={vi.fn()}
        onConfirm={vi.fn()}
        title="T"
        message="M"
        loading
      />
    )
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeDisabled()
  })
})

describe('Toast', () => {
  beforeEach(() => {
    useToastStore.setState({ toasts: [] })
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('ToastContainer renders nothing when empty', () => {
    const { container } = render(<ToastContainer toasts={[]} onClose={vi.fn()} />)
    expect(container.firstChild).toBeNull()
  })

  it('ToastContainer renders toasts of all types', () => {
    const toasts = [
      { id: '1', message: 'Success', type: 'success' as const },
      { id: '2', message: 'Error', type: 'error' as const },
      { id: '3', message: 'Warning', type: 'warning' as const },
      { id: '4', message: 'Info', type: 'info' as const },
    ]
    render(<ToastContainer toasts={toasts} onClose={vi.fn()} />)
    expect(screen.getByText('Success')).toBeInTheDocument()
    expect(screen.getByText('Error')).toBeInTheDocument()
    expect(screen.getByText('Warning')).toBeInTheDocument()
    expect(screen.getByText('Info')).toBeInTheDocument()
  })

  it('auto-dismisses after the duration', () => {
    const onClose = vi.fn()
    render(
      <ToastContainer
        toasts={[{ id: '1', message: 'Auto', type: 'info', duration: 100 }]}
        onClose={onClose}
      />
    )
    vi.advanceTimersByTime(500)
    expect(onClose).toHaveBeenCalledWith('1')
  })

  it('dismisses when the close button is clicked', () => {
    const onClose = vi.fn()
    render(
      <ToastContainer
        toasts={[{ id: '1', message: 'Dismiss me', type: 'info' }]}
        onClose={onClose}
      />
    )
    fireEvent.click(screen.getByLabelText('Dismiss notification'))
    vi.advanceTimersByTime(400)
    expect(onClose).toHaveBeenCalledWith('1')
  })

  it('store showToast and removeToast work', () => {
    useToastStore.getState().showToast('Hello', 'success', 100)
    useToastStore.getState().showToast('World')
    expect(useToastStore.getState().toasts).toHaveLength(2)
    useToastStore.getState().removeToast(useToastStore.getState().toasts[0].id)
    expect(useToastStore.getState().toasts).toHaveLength(1)
  })

  it('toast convenience functions dispatch to the store', () => {
    toast.success('S')
    toast.error('E')
    toast.warning('W')
    toast.info('I')
    const types = useToastStore.getState().toasts.map((t) => t.type)
    expect(types).toEqual(['success', 'error', 'warning', 'info'])
  })
})

describe('MetricCard', () => {
  it('renders value, unit and trend icons', () => {
    render(<MetricCard label="PnL" value="1.5" unit="$" trend="up" changePercent={5} />)
    expect(screen.getByText('PnL')).toBeInTheDocument()
    expect(screen.getByText('+5.00%')).toBeInTheDocument()
  })

  it('renders negative change formatting', () => {
    render(<MetricCard label="Loss" value={10} change={-2.5} positive={false} />)
    expect(screen.getByText('-2.50%')).toBeInTheDocument()
  })

  it('renders string change values as-is', () => {
    render(<MetricCard label="X" value={1} change="12.5%" trend="down" />)
    expect(screen.getByText('12.5%')).toBeInTheDocument()
  })

  it('determines neutral trend when changePercent is near zero', () => {
    render(<MetricCard label="Flat" value={1} changePercent={0.001} />)
    expect(screen.getByText('+0.00%')).toBeInTheDocument()
  })

  it('renders loading state without trend', () => {
    const { container } = render(
      <MetricCard label="Loading" value={100} unit="SOL" loading />
    )
    expect(screen.getByText('...')).toBeInTheDocument()
    expect(container.querySelector('.text-profit, .text-loss')).toBeNull()
  })

  it('renders with explicit neutral trend and icon', () => {
    render(<MetricCard label="N" value={5} trend="neutral" icon={<span>icon</span>} size="lg" />)
    expect(screen.getByText('icon')).toBeInTheDocument()
  })
})

describe('TimeRangePicker', () => {
  it('calls onChange for every range', () => {
    const onChange = vi.fn()
    const { container } = render(<TimeRangePicker value="24h" onChange={onChange} />)
    const buttons = container.querySelectorAll('button')
    expect(buttons).toHaveLength(7)
    fireEvent.click(buttons[0])
    expect(onChange).toHaveBeenCalledWith('1h')
    fireEvent.click(buttons[6])
    expect(onChange).toHaveBeenCalledWith('custom')
  })

  it('renders disabled', () => {
    const { container } = render(
      <TimeRangePicker value="7d" onChange={vi.fn()} disabled />
    )
    expect(container.querySelector('button')).toBeDisabled()
  })
})

describe('DateRangePicker', () => {
  it('renders dates and fires change handlers', () => {
    const onStartChange = vi.fn()
    const onEndChange = vi.fn()
    render(
      <DateRangePicker
        startDate={new Date('2025-01-01T00:00:00Z')}
        endDate={new Date('2025-01-10T00:00:00Z')}
        onStartChange={onStartChange}
        onEndChange={onEndChange}
      />
    )
    const from = document.getElementById('date-range-from') as HTMLInputElement
    const to = document.getElementById('date-range-to') as HTMLInputElement
    expect(from.value).toBe('2025-01-01')
    expect(to.value).toBe('2025-01-10')
    fireEvent.change(from, { target: { value: '2025-02-01' } })
    expect(onStartChange).toHaveBeenCalledWith(new Date('2025-02-01'))
    fireEvent.change(to, { target: { value: '' } })
    expect(onEndChange).toHaveBeenCalledWith(null)
  })

  it('renders empty dates', () => {
    render(
      <DateRangePicker
        startDate={null}
        endDate={null}
        onStartChange={vi.fn()}
        onEndChange={vi.fn()}
      />
    )
    expect((document.getElementById('date-range-from') as HTMLInputElement).value).toBe('')
  })
})

describe('RealtimeBadge', () => {
  it('renders LIVE and OFFLINE states', () => {
    render(<RealtimeBadge isLive />)
    expect(screen.getByText('LIVE')).toBeInTheDocument()
    render(<RealtimeBadge isLive={false} showText={false} />)
    expect(screen.getByText('OFFLINE')).toBeInTheDocument()
  })

  it('formats the last update time for each bucket', () => {
    const now = Date.now()
    const cases: Array<[number, string]> = [
      [now - 2 * 24 * 3600 * 1000, '2d ago'],
      [now - 2 * 3600 * 1000, '2h ago'],
      [now - 2 * 60 * 1000, '2m ago'],
      [now - 2000, '2s ago'],
      [now - 200, 'Just now'],
    ]
    for (const [ts, label] of cases) {
      render(<RealtimeBadge isLive lastUpdate={new Date(ts)} />)
      expect(screen.getByText(label)).toBeInTheDocument()
    }
  })

  it('renders no time text when there is no last update', () => {
    const { container } = render(<RealtimeBadge isLive lastUpdate={null} />)
    expect(container.textContent).not.toContain('ago')
    expect(container.textContent).not.toContain('Never')
  })
})

describe('ConnectionStatus (RealtimeBadge)', () => {
  it('colors latency by range', () => {
    const { container } = render(<ConnectionStatus connected latency={20} />)
    expect(container.textContent).toContain('20ms')
    const { container: c2 } = render(<ConnectionStatus connected latency={70} />)
    expect(c2.textContent).toContain('70ms')
    const { container: c3 } = render(<ConnectionStatus connected latency={150} />)
    expect(c3.textContent).toContain('150ms')
  })

  it('renders without latency', () => {
    const { container } = render(<ConnectionStatus connected />)
    expect(container.textContent).not.toContain('ms')
  })
})

describe('LoadingSpinner and ApiErrorBanner', () => {
  it('renders the spinner', () => {
    const { container } = render(<LoadingSpinner />)
    expect(container.querySelector('svg')).not.toBeNull()
  })

  it('ApiErrorBanner renders only when errors exist', () => {
    const { container } = render(<ApiErrorBanner errors={[null]} />)
    expect(container.firstChild).toBeNull()
    render(<ApiErrorBanner errors={[new Error('x')]} />)
    expect(screen.getByText(/API unavailable/i)).toBeInTheDocument()
  })
})
