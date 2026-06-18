import { Navigate } from 'react-router-dom'
import { useAuthStore } from '../../stores/authStore'
import type { Role } from '../../types'

interface ProtectedRouteProps {
  children: React.ReactNode
  requireRole?: Role
  redirectTo?: string
}

/**
 * Route guard component that checks authentication and role-based permissions.
 * Redirects to login if user is not authenticated.
 * Redirects to dashboard if user lacks required role.
 */
export function ProtectedRoute({
  children,
  requireRole,
  redirectTo = '/login',
}: ProtectedRouteProps) {
  const { isAuthenticated, hasPermission, isSessionExpired } = useAuthStore()

  // TEMPORARY BYPASS FOR DEBUGGING - Remove in production
  const DEV_BYPASS_AUTH = false

  // Check session expiration first
  if (!DEV_BYPASS_AUTH && isSessionExpired()) {
    // Logout will happen automatically in the API interceptor or next request
    return <Navigate to={redirectTo} replace />
  }

  // Redirect to login if not authenticated
  if (!DEV_BYPASS_AUTH && !isAuthenticated) {
    return <Navigate to={redirectTo} replace />
  }

  // Check role-based permissions
  if (!DEV_BYPASS_AUTH && requireRole && !hasPermission(requireRole)) {
    // User is authenticated but lacks required role
    // Redirect to dashboard with insufficient permissions
    return <Navigate to="/dashboard" replace />
  }

  return <>{children}</>
}
