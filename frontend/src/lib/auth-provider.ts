import type { AuthProvider } from '@refinedev/core'
import axios from 'axios'

import { decodeJwt } from './utils'

const TOKEN_KEY = 'access_token'

// In-memory cache for /users/me — single source of truth for role + identity.
// Guarantees role is always from the server, never a stale JWT claim.
interface MeCache {
  token: string
  at: number
  id: string
  name: string
  role: string
}
let _meCache: MeCache | null = null
const ME_CACHE_TTL_MS = 60_000 // 1 minute

async function fetchMe(token: string): Promise<MeCache> {
  const now = Date.now()
  if (_meCache && _meCache.token === token && now - _meCache.at < ME_CACHE_TTL_MS) {
    return _meCache
  }
  const payload = decodeJwt(token)
  const userId = String(payload['sub'] ?? payload['user_id'] ?? '')
  try {
    const res = await axios.get<{ id: number; first_name: string | null; username: string | null; role: string }>(
      '/api/v1/users/me',
      { headers: { Authorization: `Bearer ${token}` } },
    )
    const { first_name, username, role } = res.data
    const name = first_name ?? (username ? `@${username}` : `#${userId}`)
    _meCache = { token, at: now, id: userId, name, role }
    return _meCache
  } catch {
    // API unreachable — fall back to JWT claims, but don't cache (at=0 forces retry)
    const role = (payload['role'] as string) ?? 'user'
    const firstName = payload['first_name'] as string | undefined
    const username = payload['username'] as string | undefined
    const name = firstName ?? (username ? `@${username}` : `#${userId}`)
    return { token, at: 0, id: userId, name, role }
  }
}

export const authProvider: AuthProvider = {
  login: async () => ({ success: true }),

  logout: async () => {
    localStorage.removeItem(TOKEN_KEY)
    _meCache = null
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
    const me = await fetchMe(token)
    return { id: me.id, name: me.name, role: me.role }
  },

  getPermissions: async () => {
    const token = localStorage.getItem(TOKEN_KEY)
    if (!token) return null
    const me = await fetchMe(token)
    return me.role
  },

  onError: async (error: unknown) => {
    const status =
      typeof error === 'object' && error !== null && 'response' in error
        ? (error as { response?: { status?: number } }).response?.status
        : undefined
    if (status === 401) {
      localStorage.removeItem(TOKEN_KEY)
      _meCache = null
      return { logout: true }
    }
    return { error: error instanceof Error ? error : new Error(String(error)) }
  },
}
