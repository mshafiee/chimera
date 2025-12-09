
interface ConfigInputProps {
  label: string
  description?: string
  value: string | number
  onChange: ((value: string | number) => void) | ((value: number) => void) | React.Dispatch<React.SetStateAction<number>>
  type?: 'text' | 'number' | 'email' | 'password'
  min?: number
  max?: number
  step?: number
  disabled?: boolean
  error?: string
  unit?: string
  placeholder?: string
}

export function ConfigInput({
  label,
  description,
  value,
  onChange,
  type = 'text',
  min,
  max,
  step,
  disabled = false,
  error,
  unit,
  placeholder,
}: ConfigInputProps) {
  return (
    <div>
      <label className="block text-sm font-medium text-text mb-2">
        {label}
        {unit && <span className="text-text-muted ml-1">({unit})</span>}
      </label>
      <div className="relative">
        <input
          type={type}
          value={value}
          onChange={(e) => {
            if (type === 'number') {
              const numValue = e.target.value === '' ? 0 : parseFloat(e.target.value)
              const finalValue = isNaN(numValue) ? 0 : numValue
              onChange(finalValue as any)
            } else {
              onChange(e.target.value as any)
            }
          }}
          min={min}
          max={max}
          step={step}
          disabled={disabled}
          placeholder={placeholder}
          className={`w-full bg-surface border ${
            error ? 'border-loss' : 'border-border'
          } rounded-lg px-3 py-2 text-text font-mono-numbers focus:outline-none focus:ring-2 ${
            error ? 'focus:ring-loss' : 'focus:ring-shield'
          } disabled:opacity-50 disabled:cursor-not-allowed`}
        />
        {unit && (
          <span className="absolute right-3 top-1/2 -translate-y-1/2 text-text-muted text-sm">
            {unit}
          </span>
        )}
      </div>
      {error && (
        <p className="text-xs text-loss mt-1">{error}</p>
      )}
      {description && !error && (
        <p className="text-xs text-text-muted mt-1">{description}</p>
      )}
    </div>
  )
}

