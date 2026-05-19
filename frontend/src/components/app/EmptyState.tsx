import { cn } from '@core/lib/utils'

export function EmptyState({ message, className }: { message: string; className?: string }) {
  return (
    <div className={cn('flex justify-center py-12 text-sm text-muted-foreground', className)}>
      {message}
    </div>
  )
}
