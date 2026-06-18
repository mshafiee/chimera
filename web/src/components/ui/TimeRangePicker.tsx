import { clsx } from 'clsx'

export type TimeRange = '1h' | '6h' | '24h' | '7d' | '30d' | '90d' | 'custom'

interface TimeRangePickerProps {
  value: TimeRange
  onChange: (value: TimeRange) => void
  className?: string
  disabled?: boolean
}

const TIME_RANGES: { value: TimeRange; label: string }[] = [
  { value: '1h', label: '1H' },
  { value: '6h', label: '6H' },
  { value: '24h', label: '24H' },
  { value: '7d', label: '7D' },
  { value: '30d', label: '30D' },
  { value: '90d', label: '90D' },
  { value: 'custom', label: 'Custom' },
]

export function TimeRangePicker({ value, onChange, className, disabled = false }: TimeRangePickerProps) {
  return (
    <div className={clsx('flex items-center gap-1 bg-surface-light rounded-lg p-1', className)}>
      {TIME_RANGES.map((range) => (
        <button
          key={range.value}
          onClick={() => onChange(range.value)}
          disabled={disabled}
          className={clsx(
            'px-3 py-1.5 text-xs font-medium rounded transition-all',
            value === range.value
              ? 'bg-shield text-white shadow-sm'
              : 'text-text-muted hover:text-text hover:bg-surface',
            disabled && 'opacity-50 cursor-not-allowed'
          )}
        >
          {range.label}
        </button>
      ))}
    </div>
  )
}

interface DateRangePickerProps {
  startDate: Date | null
  endDate: Date | null
  onStartChange: (date: Date | null) => void
  onEndChange: (date: Date | null) => void
  className?: string
}

export function DateRangePicker({
  startDate,
  endDate,
  onStartChange,
  onEndChange,
  className,
}: DateRangePickerProps) {
  const formatDate = (date: Date | null): string => {
    if (!date) return ''
    return date.toISOString().split('T')[0]
  }

  return (
    <div className={clsx('flex items-center gap-2', className)}>
      <div className="flex items-center gap-2 bg-surface-light rounded-lg p-2">
        <div className="flex flex-col">
          <label className="text-xs text-text-muted mb-1">From</label>
          <input
            type="date"
            value={formatDate(startDate)}
            onChange={(e) => onStartChange(e.target.value ? new Date(e.target.value) : null)}
            className="text-sm bg-background border border-border rounded px-2 py-1 focus:outline-none focus:ring-1 focus:ring-shield"
          />
        </div>
        <div className="flex flex-col">
          <label className="text-xs text-text-muted mb-1">To</label>
          <input
            type="date"
            value={formatDate(endDate)}
            onChange={(e) => onEndChange(e.target.value ? new Date(e.target.value) : null)}
            className="text-sm bg-background border border-border rounded px-2 py-1 focus:outline-none focus:ring-1 focus:ring-shield"
          />
        </div>
      </div>
    </div>
  )
}
