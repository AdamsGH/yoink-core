import type { FieldValues, FieldPath, Control } from 'react-hook-form'
import { Controller } from 'react-hook-form'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@ui'
import { cn } from '@core/lib/utils'

export function ControlledSelect<T extends FieldValues>({
  name,
  options,
  control,
  className,
  onChangeTransform,
}: {
  name: FieldPath<T>
  options: { value: string; label: string }[]
  control: Control<T>
  className?: string
  onChangeTransform?: (v: string) => unknown
}) {
  return (
    <Controller
      name={name}
      control={control}
      render={({ field }) => (
        <Select
          value={String(field.value ?? '')}
          onValueChange={(v) => field.onChange(onChangeTransform ? onChangeTransform(v) : v)}
        >
          <SelectTrigger className={cn('h-8 text-xs', className ?? 'w-28')}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {options.map((o) => (
              <SelectItem key={o.value} value={o.value} className="text-xs">{o.label}</SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
    />
  )
}
