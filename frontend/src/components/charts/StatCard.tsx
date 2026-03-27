import type { ReactNode } from 'react'
import { Card, CardContent } from '@core/components/ui/card'
import { Skeleton } from '@core/components/ui/skeleton'
import { cn } from '@core/lib/utils'

type Variant = 'default' | 'success' | 'warning' | 'danger'

const variantClass: Record<Variant, string> = {
  default: 'text-primary',
  success: 'text-green-500',
  warning: 'text-yellow-500',
  danger:  'text-red-500',
}

export function StatCard({
  label,
  value,
  sub,
  icon,
  variant = 'default',
}: {
  label: string
  value: string | number | null
  sub?: string
  icon?: ReactNode
  variant?: Variant
}) {
  const display = value === null ? '-' : typeof value === 'number' ? value.toLocaleString() : value
  const isLong = typeof display === 'string' && display.length > 8
  return (
    <Card className="select-none overflow-hidden">
      <CardContent className="px-4 pt-4 pb-3">
        {icon && <div className="mb-1 text-muted-foreground">{icon}</div>}
        <div className={cn('font-bold tabular-nums truncate', isLong ? 'text-base' : 'text-2xl', variantClass[variant])}>
          {display}
        </div>
        <div className="mt-0.5 text-xs text-muted-foreground">{label}</div>
        {sub && <div className="mt-0.5 text-xs text-muted-foreground/70">{sub}</div>}
      </CardContent>
    </Card>
  )
}

export function StatCardSkeleton() {
  return (
    <Card>
      <CardContent className="px-4 pt-4 pb-3 space-y-2">
        <Skeleton className="h-7 w-20" />
        <Skeleton className="h-3 w-28" />
      </CardContent>
    </Card>
  )
}
