import { ButtonHTMLAttributes, forwardRef } from 'react'
import { clsx } from 'clsx'

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'primary' | 'secondary' | 'danger' | 'ghost' | 'shield' | 'spear'
  size?: 'sm' | 'md' | 'lg'
  loading?: boolean
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant = 'primary', size = 'md', loading, disabled, children, ...props }, ref) => {
    return (
      <button
        ref={ref}
        disabled={disabled || loading}
        className={clsx(
          'inline-flex items-center justify-center font-medium transition-all duration-200',
          'focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-offset-background',
          'disabled:opacity-50 disabled:cursor-not-allowed',
          // Variants
          {
            'bg-shield text-background hover:bg-shield-dark focus:ring-shield': variant === 'primary',
            'bg-surface border border-border text-text hover:bg-surface-light focus:ring-border': variant === 'secondary',
            'bg-loss text-white hover:bg-red-600 focus:ring-loss': variant === 'danger',
            'bg-transparent text-text hover:bg-surface focus:ring-border': variant === 'ghost',
            'bg-shield/20 text-shield border border-shield/50 hover:bg-shield/30 focus:ring-shield': variant === 'shield',
            'bg-spear/20 text-spear border border-spear/50 hover:bg-spear/30 focus:ring-spear': variant === 'spear',
          },
          // Sizes
          {
            'text-xs px-2.5 py-1.5 rounded': size === 'sm',
            'text-sm px-4 py-2 rounded-md': size === 'md',
            'text-base px-6 py-3 rounded-lg': size === 'lg',
          },
          className
        )}
        {...props}
      >
        {loading && (
          <svg
            className="animate-spin -ml-1 mr-2 h-4 w-4"
            xmlns="http://www.w3.org/2000/svg"
            fill="none"
            viewBox="0 0 24 24"
          >
            <circle
              className="opacity-25"
              cx="12"
              cy="12"
              r="10"
              stroke="currentColor"
              strokeWidth="4"
            />
            <path
              className="opacity-75"
              fill="currentColor"
              d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"
            />
          </svg>
        )}
        {children}
      </button>
    )
  }
)

Button.displayName = 'Button'
