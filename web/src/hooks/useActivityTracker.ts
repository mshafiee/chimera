import { useEffect, useRef } from 'react'
import { useAuthStore, SESSION_TIMEOUT_MS } from '../stores/authStore'

// Show warning 5 minutes before session expires
const WARNING_TIME_MS = 5 * 60 * 1000

/**
 * Hook to track user activity and manage session timeout.
 * Logs out the user after a period of inactivity.
 */
export function useActivityTracker(enabled: boolean = true) {
  const isAuthenticated = useAuthStore((s) => s.isAuthenticated)
  const timeoutRef = useRef<NodeJS.Timeout | null>(null)
  const warningTimeoutRef = useRef<NodeJS.Timeout | null>(null)

  const resetTimeout = () => {
    // Clear existing timeouts
    if (timeoutRef.current) {
      clearTimeout(timeoutRef.current)
    }
    if (warningTimeoutRef.current) {
      clearTimeout(warningTimeoutRef.current)
    }

    // Only track activity if authenticated and enabled
    if (!isAuthenticated || !enabled) {
      return
    }

    // Update activity timestamp
    useAuthStore.getState().updateActivity()

    // Set warning timeout (5 minutes before session timeout)
    warningTimeoutRef.current = setTimeout(() => {
      showSessionWarning()
    }, SESSION_TIMEOUT_MS - WARNING_TIME_MS)

    // Set session timeout
    timeoutRef.current = setTimeout(() => {
      useAuthStore.getState().logout()
      showSessionExpiredMessage()
    }, SESSION_TIMEOUT_MS)
  }

  const showSessionWarning = () => {
    console.warn('Session will expire in 5 minutes due to inactivity')

    // Optional: Show browser notification if permission granted
    if (typeof Notification !== 'undefined' && Notification.permission === 'granted') {
      new Notification('Session Expiring Soon', {
        body: 'Your session will expire in 5 minutes due to inactivity. Move your mouse or type to continue.',
        icon: '/chimera.svg',
      })
    }
  }

  const showSessionExpiredMessage = () => {
    console.warn('Session expired due to inactivity')
    if (typeof Notification !== 'undefined' && Notification.permission === 'granted') {
      new Notification('Session Expired', {
        body: 'Your session has expired due to inactivity. Please log in again.',
        icon: '/chimera.svg',
      })
    }
  }

  useEffect(() => {
    if (!isAuthenticated || !enabled) {
      return
    }

    // Request notification permission on mount
    if ('Notification' in window && Notification.permission === 'default') {
      Notification.requestPermission()
    }

    // Track various user activities
    const events = ['mousedown', 'mousemove', 'keypress', 'touchmove', 'scroll', 'click']

    const handleActivity = () => {
      resetTimeout()
    }

    // Add event listeners
    events.forEach((event) => {
      window.addEventListener(event, handleActivity)
    })

    // Initialize timeout on mount
    resetTimeout()

    // Cleanup
    return () => {
      events.forEach((event) => {
        window.removeEventListener(event, handleActivity)
      })
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current)
      }
      if (warningTimeoutRef.current) {
        clearTimeout(warningTimeoutRef.current)
      }
    }
  }, [isAuthenticated, enabled])

  // Check session status periodically (every 30 seconds)
  useEffect(() => {
    if (!isAuthenticated || !enabled) {
      return
    }

    const interval = setInterval(() => {
      if (useAuthStore.getState().isSessionExpired()) {
        useAuthStore.getState().logout()
      }
    }, 30000) // Check every 30 seconds

    return () => clearInterval(interval)
  }, [isAuthenticated, enabled])

  return {
    resetTimeout,
  }
}
