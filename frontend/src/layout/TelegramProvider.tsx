import React, { createContext, useContext, useEffect, useState } from 'react'

import { decodeJwt } from "../lib/utils"

export type CatppuccinFlavor = 'latte' | 'frappe' | 'macchiato' | 'mocha'
export type AuthState = 'loading' | 'ok' | 'error'

const DARK_FLAVORS: CatppuccinFlavor[] = ['frappe', 'macchiato', 'mocha']
const STORAGE_KEY = 'ctp-flavor'
const TOKEN_KEY = 'access_token'
const DEFAULT_FLAVOR: CatppuccinFlavor = 'macchiato'

const API_BASE = import.meta.env.VITE_API_URL ?? '/api/v1'

interface TelegramUser {
  id: number
  first_name: string
  last_name?: string
  username?: string
  language_code?: string
  photo_url?: string
}

interface TelegramContextValue {
  user: TelegramUser | null
  flavor: CatppuccinFlavor
  colorScheme: 'light' | 'dark'
  isReady: boolean
  isTelegramApp: boolean
  authState: AuthState
  setFlavor: (f: CatppuccinFlavor) => void
  toggleDark: () => void
}

const TelegramContext = createContext<TelegramContextValue>({
  user: null,
  flavor: DEFAULT_FLAVOR,
  colorScheme: 'dark',
  isReady: false,
  isTelegramApp: false,
  authState: 'loading',
  setFlavor: () => {},
  toggleDark: () => {},
})

export function useTelegram() {
  return useContext(TelegramContext)
}

function resolveInitialFlavor(): CatppuccinFlavor {
  const stored = localStorage.getItem(STORAGE_KEY) as CatppuccinFlavor | null
  if (stored && ['latte', 'frappe', 'macchiato', 'mocha'].includes(stored)) return stored
  return DEFAULT_FLAVOR
}

function applyFlavor(flavor: CatppuccinFlavor) {
  const root = document.documentElement
  root.setAttribute('data-theme', flavor)
  root.classList.toggle('dark', DARK_FLAVORS.includes(flavor))
  localStorage.setItem(STORAGE_KEY, flavor)
}

function isTokenValid(token: string): boolean {
  const payload = decodeJwt(token)
  const exp = payload['exp'] as number | undefined
  if (!exp) return false
  return Date.now() / 1000 < exp
}

function getDevParam(): string | null {
  return (
    new URLSearchParams(window.location.search).get('dev_token') ??
    (import.meta.env.VITE_DEV_TOKEN as string | undefined) ??
    null
  )
}

async function exchangeTelegramUser(initData: string): Promise<string> {
  const res = await fetch(`${API_BASE}/auth/token`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ init_data: initData }),
  })
  if (!res.ok) throw new Error(`auth/token ${res.status}`)
  const data = (await res.json()) as { access_token: string }
  return data.access_token
}

async function exchangeDevToken(devToken: string): Promise<string> {
  // devToken format: "<user_id>:<role>" e.g. "123456789:owner"
  const [userId, role = 'user'] = devToken.split(':')
  const params = new URLSearchParams({ user_id: userId, role })
  const res = await fetch(`${API_BASE}/auth/dev?${params}`, { method: 'POST' })
  if (!res.ok) throw new Error(`auth/dev ${res.status}`)
  const data = (await res.json()) as { access_token: string }
  return data.access_token
}

export function TelegramProvider({ children }: { children: React.ReactNode }) {
  const [isReady, setIsReady] = useState(false)
  const [user, setUser] = useState<TelegramUser | null>(null)
  const [flavor, setFlavorState] = useState<CatppuccinFlavor>(resolveInitialFlavor)
  const [authState, setAuthState] = useState<AuthState>('loading')

  useEffect(() => {
    applyFlavor(flavor)
  }, [flavor])

  useEffect(() => {
    async function init() {
      const tg = window.Telegram?.WebApp
      const devToken = getDevParam()

      // Apply flavor from Telegram color scheme
      if (tg) {
        tg.ready()
        tg.expand()
        try { tg.disableVerticalSwipes?.() } catch { /* old SDK */ }

        if (tg.colorScheme === 'light') {
          setFlavorState('latte')
        } else {
          const stored = localStorage.getItem(STORAGE_KEY) as CatppuccinFlavor | null
          if (stored && DARK_FLAVORS.includes(stored)) setFlavorState(stored)
          else setFlavorState(DEFAULT_FLAVOR)
        }

        if (tg.initDataUnsafe?.user) {
          setUser(tg.initDataUnsafe.user as TelegramUser)
        }
      }

      // Try existing valid token first
      const cached = localStorage.getItem(TOKEN_KEY)
      if (cached && isTokenValid(cached)) {
        setAuthState('ok')
        setIsReady(true)
        return
      }

      // Exchange for a new token
      try {
        let token: string
        const tgInitData = tg?.initData

        if (devToken) {
          token = await exchangeDevToken(devToken)
        } else if (tgInitData) {
          token = await exchangeTelegramUser(tgInitData)
        } else {
          setAuthState('error')
          setIsReady(true)
          return
        }

        localStorage.setItem(TOKEN_KEY, token)
        setAuthState('ok')

        // Load theme from backend and apply it
        try {
          const settingsRes = await fetch(`${API_BASE}/settings`, {
            headers: { Authorization: `Bearer ${token}` },
          })
          if (settingsRes.ok) {
            const s = (await settingsRes.json()) as { theme?: string }
            if (s.theme && ['latte', 'frappe', 'macchiato', 'mocha'].includes(s.theme)) {
              const dbFlavor = s.theme as CatppuccinFlavor
              setFlavorState(dbFlavor)
              applyFlavor(dbFlavor)
            }
          }
        } catch {
          // Non-critical: theme stays as localStorage/default
        }
      } catch (err: unknown) {
        const msg = err instanceof Error ? err.message : String(err)
        // auth failed - keep error state, alert in dev mode below
        localStorage.removeItem(TOKEN_KEY)
        setAuthState('error')
        // Show error reason on screen for debugging
        if (import.meta.env.DEV || getDevParam()) {
          alert(`Auth failed: ${msg}`)
        }
      }

      setIsReady(true)
    }

    init()
  }, [])

  const setFlavor = (f: CatppuccinFlavor) => {
    setFlavorState(f)
    applyFlavor(f)
    // Sync theme to backend (fire-and-forget)
    const token = localStorage.getItem(TOKEN_KEY)
    if (token) {
      fetch(`${API_BASE}/settings`, {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` },
        body: JSON.stringify({ theme: f }),
      }).catch(() => {/* non-critical */})
    }
  }

  const toggleDark = () => {
    const isDark = DARK_FLAVORS.includes(flavor)
    setFlavor(isDark ? 'latte' : 'macchiato')
  }

  const colorScheme: 'light' | 'dark' = DARK_FLAVORS.includes(flavor) ? 'dark' : 'light'
  const isTelegramApp = Boolean(window.Telegram?.WebApp?.initData)

  return (
    <TelegramContext.Provider
      value={{ user, flavor, colorScheme, isReady, isTelegramApp, authState, setFlavor, toggleDark }}
    >
      {children}
    </TelegramContext.Provider>
  )
}
