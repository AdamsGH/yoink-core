import type { FieldValues, FieldPath, Control } from 'react-hook-form'
import { Controller } from 'react-hook-form'
import { Switch } from '@ui'
import { SettingRow } from '@core/components/app/SettingRow'

export function ControlledSwitch<T extends FieldValues>({
  name,
  label,
  hint,
  control,
}: {
  name: FieldPath<T>
  label: string
  hint?: string
  control: Control<T>
}) {
  return (
    <Controller
      name={name}
      control={control}
      render={({ field }) => (
        <SettingRow label={label} hint={hint}>
          <Switch checked={!!field.value} onCheckedChange={field.onChange} />
        </SettingRow>
      )}
    />
  )
}
