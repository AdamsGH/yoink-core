import { Badge } from '@core/components/ui/badge'
import { cn } from '@core/lib/utils'
import type { UserRole } from '@core/types/api'

// Extended className presets — never touch badge.tsx variants
const SUCCESS = 'border-transparent bg-[var(--ctp-green,#40a02b)]/20 text-[var(--ctp-green,#40a02b)]'
const WARNING = 'border-transparent bg-[var(--ctp-yellow,#df8e1d)]/20 text-[var(--ctp-yellow,#df8e1d)]'

// Cookie valid/invalid
export function CookieStatusBadge({ valid, className }: { valid: boolean; className?: string }) {
  return (
    <Badge
      className={cn(
        'border-transparent',
        valid ? SUCCESS : 'bg-destructive/20 text-destructive',
        className
      )}
    >
      {valid ? 'Valid' : 'Invalid'}
    </Badge>
  )
}

// Generic success/warning badges for reuse
export function SuccessBadge({ children, className }: { children: React.ReactNode; className?: string }) {
  return <Badge className={cn(SUCCESS, className)}>{children}</Badge>
}

export function WarningBadge({ children, className }: { children: React.ReactNode; className?: string }) {
  return <Badge className={cn(WARNING, className)}>{children}</Badge>
}

// User role colored badge
const ROLE_STYLES: Record<UserRole, string> = {
  owner:      'bg-[var(--ctp-mauve,#cba6f7)]/20 text-[var(--ctp-mauve,#cba6f7)]',
  admin:      'bg-[var(--ctp-red,#f38ba8)]/20   text-[var(--ctp-red,#f38ba8)]',
  moderator:  'bg-[var(--ctp-blue,#89b4fa)]/20  text-[var(--ctp-blue,#89b4fa)]',
  user:       'bg-secondary text-secondary-foreground',
  restricted: 'bg-[var(--ctp-peach,#fab387)]/20 text-[var(--ctp-peach,#fab387)]',
  banned:     'bg-destructive/20 text-destructive',
}

export function RoleBadge({ role, className }: { role: UserRole; className?: string }) {
  return (
    <Badge className={cn('border-transparent capitalize', ROLE_STYLES[role] ?? ROLE_STYLES.user, className)}>
      {role}
    </Badge>
  )
}
