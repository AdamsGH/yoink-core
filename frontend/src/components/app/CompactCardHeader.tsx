import type { ReactNode } from 'react'
import { CardHeader, CardTitle } from '@ui'
import { cn } from '@core/lib/utils'

/**
 * CardHeader with compact padding (px-4 py-3) and base-size title.
 * Drop-in replacement for the repeated pattern across admin pages.
 */
export function CompactCardHeader({
  title,
  actions,
  className,
}: {
  title: ReactNode
  actions?: ReactNode
  className?: string
}) {
  return (
    <CardHeader className={cn('px-4 py-3', className)}>
      <div className="flex items-center justify-between gap-2">
        <CardTitle className="flex items-center gap-2 text-base">{title}</CardTitle>
        {actions && <div className="flex items-center gap-1.5 shrink-0">{actions}</div>}
      </div>
    </CardHeader>
  )
}
