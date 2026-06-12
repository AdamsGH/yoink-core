import { cn } from '@core/lib/utils'

/**
 * Default constrained page wrapper. Wrap the root element of any page that
 * renders inside AppLayout's <Outlet> and does NOT need to fill the full
 * available width. Pages that manage their own layout (e.g. split-pane,
 * sidebar+grid) should NOT use this and instead set h-full on their root.
 */
export function PageContainer({
  children,
  className,
}: {
  children: React.ReactNode
  className?: string
}) {
  return (
    <div className={cn('mx-auto w-full max-w-4xl px-4 py-6', className)}>
      {children}
    </div>
  )
}
