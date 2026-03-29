import { apiClient } from '@core/lib/api-client'
import type { UserRole } from '@core/types/api'

/** Gradient classes for role-based Drawer headers. */
export const GRADIENT_BY_ROLE: Record<string, string> = {
  owner:      'from-amber-500/30 via-amber-500/10 to-transparent',
  admin:      'from-blue-500/30 via-blue-500/10 to-transparent',
  moderator:  'from-purple-500/25 via-purple-500/8 to-transparent',
  user:       'from-primary/20 via-primary/8 to-transparent',
  restricted: 'from-orange-500/25 via-orange-500/8 to-transparent',
  banned:     'from-destructive/25 via-destructive/8 to-transparent',
}

/** Ring colour classes for role-based Avatar borders. */
export const RING_BY_ROLE: Record<string, string> = {
  owner:      'ring-amber-500/60',
  admin:      'ring-blue-500/60',
  moderator:  'ring-purple-500/60',
  user:       'ring-primary/40',
  restricted: 'ring-orange-500/60',
  banned:     'ring-destructive/60',
}

/** Text colour for role-based Avatar fallback initials. */
export function roleMediaColor(role: UserRole | string): string {
  switch (role) {
    case 'owner':      return 'text-amber-500'
    case 'admin':      return 'text-blue-500'
    case 'moderator':  return 'text-purple-500'
    case 'restricted': return 'text-orange-500'
    case 'banned':     return 'text-destructive'
    default:           return 'text-primary'
  }
}

/** Two-letter initials from first_name or username. */
export function userInitials(user: { first_name?: string | null; username?: string | null }): string {
  const name = user.first_name ?? user.username ?? ''
  return name.slice(0, 2).toUpperCase() || '#'
}

/** Absolute URL for a user's proxied Telegram photo. */
export function userPhotoUrl(userId: number): string {
  return `${apiClient.defaults.baseURL}/users/${userId}/photo`
}

/** Open a Telegram profile link via WebApp or browser fallback. */
export function openProfileLink(userId: number, username: string | null): void {
  const tg = window.Telegram?.WebApp
  if (username) {
    const url = `https://t.me/${username}`
    tg ? tg.openTelegramLink(url) : window.open(url, '_blank')
  } else {
    tg ? tg.openTelegramLink(`tg://user?id=${userId}`) : window.open(`tg://user?id=${userId}`)
  }
}
