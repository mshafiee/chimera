import { ReactNode } from 'react'

interface ConfigToggleProps {
  label: string
  description?: string
  enabled: boolean
  onChange: (enabled: boolean) => void
  disabled?: boolean
  badge?: ReactNode
}

export function ConfigToggle({
  label,
  description,
  enabled,
  onChange,
  disabled = false,
  badge,
}: ConfigToggleProps) {
  return (
    <div className="flex items-start justify-between gap-4">
      <div className="flex-1">
        <div className="flex items-center gap-2 mb-1">
          <label className="text-sm font-medium text-text cursor-pointer">
            {label}
          </label>
          {badge}
        </div>
        {description && (
          <p className="text-xs text-text-muted">{description}</p>
        )}
      </div>
      <button
        type="button"
        onClick={() => !disabled && onChange(!enabled)}
        disabled={disabled}
        className={`relative inline-flex h-6 w-11 flex-shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors duration-200 ease-in-out focus:outline-none focus:ring-2 focus:ring-shield focus:ring-offset-2 ${
          enabled ? 'bg-shield' : 'bg-surface-light'
        } ${disabled ? 'opacity-50 cursor-not-allowed' : ''}`}
        role="switch"
        aria-checked={enabled}
      >
        <span
          className={`pointer-events-none inline-block h-5 w-5 transform rounded-full bg-white shadow ring-0 transition duration-200 ease-in-out ${
            enabled ? 'translate-x-5' : 'translate-x-0'
          }`}
        />
      </button>
    </div>
  )
}

