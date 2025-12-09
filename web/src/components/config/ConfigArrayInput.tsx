import { X, Plus } from 'lucide-react'
import { Button } from '../ui/Button'

interface ConfigArrayInputProps {
  label: string
  description?: string
  values: number[]
  onChange: (values: number[]) => void
  disabled?: boolean
  min?: number
  max?: number
  unit?: string
  placeholder?: string
}

export function ConfigArrayInput({
  label,
  description,
  values,
  onChange,
  disabled = false,
  min,
  max,
  unit,
  placeholder,
}: ConfigArrayInputProps) {
  const addItem = () => {
    const lastValue = values.length > 0 ? values[values.length - 1] : 0
    onChange([...values, lastValue + 25])
  }

  const removeItem = (index: number) => {
    onChange(values.filter((_, i) => i !== index))
  }

  const updateItem = (index: number, value: number) => {
    const newValues = [...values]
    newValues[index] = value
    onChange(newValues)
  }

  return (
    <div>
      <label className="block text-sm font-medium text-text mb-2">
        {label}
        {unit && <span className="text-text-muted ml-1">({unit})</span>}
      </label>
      {description && (
        <p className="text-xs text-text-muted mb-3">{description}</p>
      )}
      <div className="space-y-2">
        {values.map((value, index) => (
          <div key={index} className="flex items-center gap-2">
            <input
              type="number"
              value={value}
              onChange={(e) => {
                const numValue = parseFloat(e.target.value) || 0
                if (min !== undefined && numValue < min) return
                if (max !== undefined && numValue > max) return
                updateItem(index, numValue)
              }}
              min={min}
              max={max}
              disabled={disabled}
              placeholder={placeholder}
              className="flex-1 bg-surface border border-border rounded-lg px-3 py-2 text-text font-mono-numbers focus:outline-none focus:ring-2 focus:ring-shield disabled:opacity-50"
            />
            {unit && (
              <span className="text-sm text-text-muted w-8">{unit}</span>
            )}
            {!disabled && (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => removeItem(index)}
                className="text-loss hover:text-loss hover:bg-loss/10"
              >
                <X className="w-4 h-4" />
              </Button>
            )}
          </div>
        ))}
        {!disabled && (
          <Button
            variant="ghost"
            size="sm"
            onClick={addItem}
            className="w-full text-text-muted hover:text-text"
          >
            <Plus className="w-4 h-4 mr-2" />
            Add Target
          </Button>
        )}
      </div>
    </div>
  )
}

