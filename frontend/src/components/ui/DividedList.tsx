import type { ReactNode } from 'react'
import { cn } from '@core/lib/utils'

export function DividedList({
  children,
  className,
  style,
}: {
  children: ReactNode
  className?: string
  style?: React.CSSProperties
}) {
  return (
    <div className={cn('divide-y divide-border px-3 py-1', className)} style={style}>
      {children}
    </div>
  )
}
