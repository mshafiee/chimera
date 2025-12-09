import { useState, ReactNode } from 'react'
import { ChevronDown, ChevronUp } from 'lucide-react'
import { Card, CardHeader, CardTitle, CardContent } from '../ui/Card'

interface ConfigSectionProps {
  title: string
  description?: string
  defaultOpen?: boolean
  collapsible?: boolean
  children: ReactNode
  icon?: ReactNode
  badge?: ReactNode
}

export function ConfigSection({
  title,
  description,
  defaultOpen = true,
  collapsible = true,
  children,
  icon,
  badge,
}: ConfigSectionProps) {
  const [isOpen, setIsOpen] = useState(defaultOpen)

  return (
    <Card>
      <CardHeader
        className="cursor-pointer"
        onClick={() => collapsible && setIsOpen(!isOpen)}
      >
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            {icon && <div className="text-text-muted">{icon}</div>}
            <CardTitle className="text-base md:text-lg">{title}</CardTitle>
            {badge}
          </div>
          {collapsible && (
            <div className="text-text-muted">
              {isOpen ? (
                <ChevronUp className="w-5 h-5" />
              ) : (
                <ChevronDown className="w-5 h-5" />
              )}
            </div>
          )}
        </div>
        {description && (
          <p className="text-sm text-text-muted mt-2">{description}</p>
        )}
      </CardHeader>
      {isOpen && <CardContent>{children}</CardContent>}
    </Card>
  )
}

