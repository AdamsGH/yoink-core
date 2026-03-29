import { cn } from '@core/lib/utils'
import { Button } from '@ui'

export const PERIOD_OPTIONS = [
  { label: '7d',  value: 7  },
  { label: '30d', value: 30 },
  { label: '90d', value: 90 },
  { label: 'All', value: 0  },
] as const

export type Period = (typeof PERIOD_OPTIONS)[number]['value']

export function PeriodToggle({
  value,
  onChange,
}: {
  value: Period
  onChange: (v: Period) => void
}) {
  return (
    <div className="flex rounded-md border">
      {PERIOD_OPTIONS.map((opt) => (
        <Button
          key={opt.value}
          variant="ghost"
          size="sm"
          onClick={() => onChange(opt.value)}
          className={cn(
            'h-7 rounded-none px-2.5 text-xs first:rounded-l-md last:rounded-r-md',
            value === opt.value && 'bg-muted font-semibold',
          )}
        >
          {opt.label}
        </Button>
      ))}
    </div>
  )
}
