import * as React from 'react'
import { Check, ChevronsUpDown } from 'lucide-react'
import { cn } from '@core/lib/utils'
import { Button } from '@core/components/ui/button'
import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from '@core/components/ui/command'
import { Popover, PopoverContent, PopoverTrigger } from '@core/components/ui/popover'

export interface InlineSelectOption {
  value: string
  label: string
  meta?: string
}

interface InlineSelectProps {
  options: InlineSelectOption[]
  value: string
  onValueChange: (value: string, option: InlineSelectOption) => void
  placeholder?: string
  searchPlaceholder?: string
  emptyText?: string
  className?: string
}

export function InlineSelect({
  options,
  value,
  onValueChange,
  placeholder = 'Select...',
  searchPlaceholder = 'Search...',
  emptyText = 'No results found.',
  className,
}: InlineSelectProps) {
  const [open, setOpen] = React.useState(false)
  const selected = options.find((o) => o.value === value)

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          role="combobox"
          aria-expanded={open}
          className={cn('w-full justify-between font-normal', className)}
        >
          <span className="truncate">{selected?.label ?? placeholder}</span>
          <ChevronsUpDown className="ml-2 h-4 w-4 shrink-0 opacity-50" />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        className="p-0"
        style={{ width: 'var(--radix-popover-trigger-width)' }}
        align="start"
      >
        <Command>
          <CommandInput placeholder={searchPlaceholder} />
          <CommandList onWheel={(e) => e.stopPropagation()}>
            <CommandEmpty>{emptyText}</CommandEmpty>
            <CommandGroup>
              {options.map((option) => (
                <CommandItem
                  key={option.value}
                  value={`${option.label} ${option.meta ?? ''}`}
                  onSelect={() => {
                    onValueChange(option.value, option)
                    setOpen(false)
                  }}
                >
                  <Check className={cn('mr-2 h-4 w-4 shrink-0', option.value === value ? 'opacity-100' : 'opacity-0')} />
                  <span className="flex-1 truncate">{option.label}</span>
                  {option.meta && (
                    <span className="ml-2 font-mono text-xs text-muted-foreground">{option.meta}</span>
                  )}
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  )
}
