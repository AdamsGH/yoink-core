import { Component, type ReactNode } from 'react'
import { BrowserRouter, Navigate, Route, Routes } from 'react-router'
import { Refine } from '@refinedev/core'
import { NavigateToResource } from '@refinedev/react-router'
import { Toaster } from 'sonner'

import { authProvider } from './lib/auth-provider'
import { dataProvider } from './lib/data-provider'
import { AppLayout } from './layout/AppLayout'
import { useTelegram } from './layout/TelegramProvider'
import { usePermissions } from './hooks/usePermissions'
import { Button } from './components/ui/button'
import { ProtectedRoute } from './components/ProtectedRoute'
import { plugins } from './plugin-registry'
import type { NavGroup, PluginManifest, PluginRoute } from './types/plugin'

import UnauthorizedPage from './pages/unauthorized'

function buildNavGroups(ps: PluginManifest[]): NavGroup[] {
  const all = ps.flatMap((p) => p.navGroups ?? [])
  // Merge groups with the same label. Unlabelled groups all merge into one.
  const merged = new Map<string, NavGroup>()
  let unlabelled: NavGroup | null = null
  for (const g of all) {
    if (!g.label) {
      if (!unlabelled) unlabelled = { items: [...g.items] }
      else unlabelled.items = [...unlabelled.items, ...g.items]
      continue
    }
    const existing = merged.get(g.label)
    if (existing) {
      existing.items = [...existing.items, ...g.items]
    } else {
      merged.set(g.label, { ...g, items: [...g.items] })
    }
  }
  return [...(unlabelled ? [unlabelled] : []), ...merged.values()]
}

function resolveStatsEndpoint(ps: PluginManifest[]): string | undefined {
  return ps.find((p) => p.userStatsEndpoint)?.userStatsEndpoint
}

function AuthGate({ children }: { children: ReactNode }) {
  const { authState, isTelegramApp } = useTelegram()

  if (authState === 'loading') {
    return (
      <div className="flex min-h-screen items-center justify-center bg-background">
        <div className="flex flex-col items-center gap-3 text-muted-foreground">
          <div className="h-7 w-7 animate-spin rounded-full border-2 border-border border-t-primary" />
          <span className="text-sm">Signing in…</span>
        </div>
      </div>
    )
  }

  if (authState === 'error') {
    return (
      <div className="flex min-h-screen items-center justify-center bg-background p-6">
        <div className="flex flex-col items-center gap-2 text-center">
          <p className="text-base font-medium text-foreground">
            {isTelegramApp ? 'Authentication failed' : 'Open in Telegram'}
          </p>
          <p className="max-w-xs text-sm text-muted-foreground">
            {isTelegramApp
              ? 'Could not sign in. Please close and reopen the app.'
              : 'This app must be launched from the Telegram bot as a Mini App.'}
          </p>
          {isTelegramApp && (
            <Button className="mt-3" onClick={() => window.location.reload()}>Retry</Button>
          )}
        </div>
      </div>
    )
  }

  return <>{children}</>
}

function _guardRoute(r: PluginRoute): ReactNode {
  if (!r.minRole) return r.element
  return <ProtectedRoute minRole={r.minRole}>{r.element}</ProtectedRoute>
}

const ROLE_WEIGHT: Record<string, number> = {
  owner: 0, admin: 1, moderator: 2, user: 3, restricted: 4, banned: 5,
}

function firstAccessiblePath(navGroups: NavGroup[], role: string | null): string {
  for (const g of navGroups) {
    if (g.minRole) {
      const w = ROLE_WEIGHT[role ?? ''] ?? 99
      const allowed = Array.isArray(g.minRole) ? g.minRole : [g.minRole]
      if (!allowed.some((r) => w <= (ROLE_WEIGHT[r] ?? 99))) continue
    }
    for (const item of g.items) {
      if (!item.minRole) return item.path
      const w = ROLE_WEIGHT[role ?? ''] ?? 99
      const allowed = Array.isArray(item.minRole) ? item.minRole : [item.minRole]
      if (allowed.some((r) => w <= (ROLE_WEIGHT[r] ?? 99))) return item.path
    }
  }
  return '/'
}

function RefineApp() {
  const navGroups     = buildNavGroups(plugins)
  const resources     = plugins.flatMap((p) => p.resources ?? [])
  const routes        = plugins.flatMap((p) => p.routes)
  const statsEndpoint = resolveStatsEndpoint(plugins)
  const { role, isLoading: roleLoading } = usePermissions()

  return (
    <BrowserRouter>
      <Refine
        dataProvider={dataProvider}
        authProvider={authProvider}
        resources={resources}
        options={{ syncWithLocation: true, warnWhenUnsavedChanges: true, disableTelemetry: true }}
      >
        <Routes>
          <Route element={<AppLayout navGroups={navGroups} userStatsEndpoint={statsEndpoint} />}>
            <Route index element={
              roleLoading
                ? <div className="flex min-h-[60vh] items-center justify-center"><div className="h-5 w-5 animate-spin rounded-full border-2 border-border border-t-primary" /></div>
                : <Navigate to={firstAccessiblePath(navGroups, role)} replace />
            } />
            {routes.map((r) => (
              <Route
                key={r.path}
                path={r.path}
                element={_guardRoute(r)}
              />
            ))}
          </Route>
          <Route path="/unauthorized" element={<UnauthorizedPage />} />
          <Route path="*" element={<NavigateToResource resource={resources[0]?.name ?? 'settings'} />} />
        </Routes>
      </Refine>
    </BrowserRouter>
  )
}

class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null }
  static getDerivedStateFromError(error: Error) { return { error } }
  render() {
    if (this.state.error) {
      return (
        <div style={{ padding: 24, color: 'red', fontFamily: 'monospace', whiteSpace: 'pre-wrap' }}>
          <h2>App crashed</h2>
          <p>{this.state.error.message}</p>
          <pre>{this.state.error.stack}</pre>
        </div>
      )
    }
    return this.props.children
  }
}

export default function App() {
  return (
    <ErrorBoundary>
      <AuthGate>
        <RefineApp />
      </AuthGate>
      <Toaster position="top-right" richColors />
    </ErrorBoundary>
  )
}
