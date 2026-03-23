import type { ReactNode } from 'react'
import { Navigate } from 'react-router'

import { usePermissions } from '@/hooks/usePermissions'

interface ProtectedRouteProps {
  minRole: string
  children: ReactNode
}

/**
 * Wraps a route element and redirects to /unauthorized if the current
 * user's role is below the required minimum.
 *
 * Usage:
 *   <ProtectedRoute minRole="admin">
 *     <AdminUsersPage />
 *   </ProtectedRoute>
 */
export function ProtectedRoute({ minRole, children }: ProtectedRouteProps) {
  const { hasRole, isLoading } = usePermissions()

  if (isLoading) {
    return (
      <div className="flex min-h-[60vh] items-center justify-center">
        <div className="h-6 w-6 animate-spin rounded-full border-2 border-border border-t-primary" />
      </div>
    )
  }

  if (!hasRole(minRole)) {
    return <Navigate to="/unauthorized" replace />
  }

  return <>{children}</>
}
