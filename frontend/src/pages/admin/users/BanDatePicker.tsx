import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { CalendarIcon, X } from 'lucide-react'

import { cn, formatDate } from '@core/lib/utils'
import { Button, Calendar, Label, Popover, PopoverContent, PopoverTrigger, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@ui'

export function BanDatePicker({
  value,
  onChange,
}: {
  value: string
  onChange: (iso: string) => void
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const [hour, setHour] = useState('23')
  const [minute, setMinute] = useState('59')

  const selected = value ? new Date(value) : undefined

  const applyTime = (day: Date, h: string, m: string) => {
    const d = new Date(day)
    d.setHours(parseInt(h, 10), parseInt(m, 10), 0, 0)
    onChange(d.toISOString())
  }

  const handleDaySelect = (day: Date | undefined) => {
    if (!day) { onChange(''); setOpen(false); return }
    applyTime(day, hour, minute)
    setOpen(false)
  }

  const handleHourChange = (h: string) => {
    setHour(h)
    if (selected) applyTime(selected, h, minute)
  }

  const handleMinuteChange = (m: string) => {
    setMinute(m)
    if (selected) applyTime(selected, hour, m)
  }

  const label = selected
    ? formatDate(selected.toISOString())
    : t('users.ban_until_placeholder', { defaultValue: 'Pick date…' })

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          className={cn('w-full justify-start gap-2 font-normal', !selected && 'text-muted-foreground')}
        >
          <CalendarIcon className="h-4 w-4 shrink-0" />
          <span className="flex-1 text-left truncate">{label}</span>
          {selected && (
            <X
              className="h-3.5 w-3.5 shrink-0 opacity-50 hover:opacity-100"
              onClick={(e) => { e.stopPropagation(); onChange('') }}
            />
          )}
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-auto p-0" align="start">
        <Calendar
          mode="single"
          selected={selected}
          onSelect={handleDaySelect}
          disabled={(d) => d < new Date()}
          initialFocus
        />
        <div className="border-t p-3 flex items-center gap-2">
          <Label className="text-xs text-muted-foreground shrink-0">
            {t('users.ban_until_time', { defaultValue: 'Time' })}
          </Label>
          <Select value={hour} onValueChange={handleHourChange}>
            <SelectTrigger className="h-8 w-20">
              <SelectValue />
            </SelectTrigger>
            <SelectContent className="max-h-48">
              {Array.from({ length: 24 }, (_, i) => String(i).padStart(2, '0')).map(h => (
                <SelectItem key={h} value={h}>{h}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          <span className="text-muted-foreground">:</span>
          <Select value={minute} onValueChange={handleMinuteChange}>
            <SelectTrigger className="h-8 w-20">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {['00', '15', '30', '45'].map(m => (
                <SelectItem key={m} value={m}>{m}</SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </PopoverContent>
    </Popover>
  )
}
