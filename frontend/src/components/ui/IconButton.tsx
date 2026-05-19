import type { ReactNode } from 'react'
import { Button } from './button'
import { Tooltip, TooltipContent, TooltipTrigger } from './tooltip'
import { cn } from '@core/lib/utils'

export function IconButton({
  tooltip,
  onClick,
  disabled,
  children,
  className,
  variant = 'ghost',
}: {
  tooltip: string
  onClick?: () => void
  disabled?: boolean
  children: ReactNode
  className?: string
  variant?: 'ghost' | 'outline' | 'destructive'
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant={variant}
          size="icon"
          className={cn('h-8 w-8', className)}
          onClick={onClick}
          disabled={disabled}
        >
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent>{tooltip}</TooltipContent>
    </Tooltip>
  )
}
