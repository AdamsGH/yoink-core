import { usePermissions as useRefinePermissions } from '@refinedev/core'

const ROLE_ORDER: Record<string, number> = {
  banned: 0,
  restricted: 1,
  user: 2,
  moderator: 3,
  admin: 4,
  owner: 5,
}

function rankOf(role: string | null | undefined): number {
  return ROLE_ORDER[role ?? ''] ?? -1
}

export interface PermissionState {
  role: string | null
  isLoading: boolean
  hasRole: (minRole: string) => boolean
  isAdmin: boolean
  isModerator: boolean
  isOwner: boolean
}

export function usePermissions(): PermissionState {
  const { data: role, isLoading } = useRefinePermissions<string>({})

  const hasRole = (minRole: string): boolean => {
    if (!role) return false
    return rankOf(role) >= rankOf(minRole)
  }

  return {
    role: role ?? null,
    isLoading,
    hasRole,
    isAdmin: hasRole('admin'),
    isModerator: hasRole('moderator'),
    isOwner: role === 'owner',
  }
}
