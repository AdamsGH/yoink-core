import type { ReactNode } from 'react'
import { cn } from '@core/lib/utils'

interface SettingRowProps {
  label: string
  hint?: string
  children: ReactNode
  /**
   * 'inline' (default) - label on left, control on right; always horizontal.
   *   Used in mobile-first Telegram WebApp settings pages.
   * 'stacked' - stacks vertically on mobile, horizontal on sm+ with wide label.
   *   Used in admin bot-settings with multi-line controls.
   */
  variant?: 'inline' | 'stacked'
  className?: string
}

export function SettingRow({ label, hint, children, variant = 'inline', className }: SettingRowProps) {
  if (variant === 'stacked') {
    return (
      <div className={cn('flex flex-col gap-1.5 sm:flex-row sm:items-start sm:gap-4', className)}>
        <div className="sm:w-48 shrink-0 pt-0.5">
          <p className="text-sm font-medium leading-none">{label}</p>
          {hint && <p className="mt-1 text-xs text-muted-foreground leading-snug">{hint}</p>}
        </div>
        <div className="flex-1 min-w-0">{children}</div>
      </div>
    )
  }

  return (
    <div className={cn('flex items-center justify-between gap-4 py-2.5', className)}>
      <div className="min-w-0 flex-1">
        <p className="text-sm">{label}</p>
        {hint && <p className="mt-0.5 text-xs text-muted-foreground">{hint}</p>}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  )
}
