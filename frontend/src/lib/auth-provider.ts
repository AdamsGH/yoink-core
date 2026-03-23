import type { AuthProvider } from '@refinedev/core'
import axios from 'axios'

import { decodeJwt } from './utils'

const TOKEN_KEY = 'access_token'

export const authProvider: AuthProvider = {
  // Login is handled by TelegramProvider before Refine mounts.
  // This is a no-op kept for Refine compatibility.
  login: async () => ({ success: true }),

  logout: async () => {
    localStorage.removeItem(TOKEN_KEY)
    return { success: true }
  },

  check: async () => {
    const token = localStorage.getItem(TOKEN_KEY)
    if (!token) return { authenticated: false }

    const payload = decodeJwt(token)
    const exp = payload['exp'] as number | undefined
    if (exp && Date.now() / 1000 > exp) {
      localStorage.removeItem(TOKEN_KEY)
      return { authenticated: false }
    }

    return { authenticated: true }
  },

  getIdentity: async () => {
    const token = localStorage.getItem(TOKEN_KEY)
    if (!token) return null

    const payload = decodeJwt(token)
    const userId = payload['sub'] ?? payload['user_id']
    const role = payload['role'] as string | undefined

    const firstName = payload['first_name'] as string | undefined
    const username = payload['username'] as string | undefined

    // Fast path: name is embedded in the token
    if (firstName ?? username) {
      return {
        id: userId,
        name: firstName ?? `@${username}`,
        role,
      }
    }

    // Fallback: fetch from API (old tokens without name fields)
    try {
      const res = await axios.get<{ first_name: string | null; username: string | null }>(
        '/api/v1/users/me',
        { headers: { Authorization: `Bearer ${token}` } },
      )
      const { first_name, username: uname } = res.data
      const name = first_name ?? (uname ? `@${uname}` : `#${userId}`)
      return { id: userId, name, role }
    } catch (err) {
      console.error('Failed to fetch user identity', err)
      return { id: userId, name: `#${userId}`, role }
    }
  },

  getPermissions: async () => {
    const token = localStorage.getItem(TOKEN_KEY)
    if (!token) return null
    const payload = decodeJwt(token)
    return payload['role'] ?? null
  },

  onError: async (error: unknown) => {
    const status =
      typeof error === 'object' && error !== null && 'response' in error
        ? (error as { response?: { status?: number } }).response?.status
        : undefined
    if (status === 401) {
      localStorage.removeItem(TOKEN_KEY)
      return { logout: true }
    }
    return { error: error instanceof Error ? error : new Error(String(error)) }
  },
}
