import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { NavLink, Outlet, useLocation, useNavigate } from 'react-router'
import { ChevronDown, Menu, Moon, Palette, Shield, Sun, X } from 'lucide-react'

import { usePermissions } from '@/hooks/usePermissions'
import { apiClient } from '@core/lib/api-client'
import { useTelegram, type CatppuccinFlavor } from './TelegramProvider'
import { UserPanel } from '../components/app/UserPanel'
import { Button } from '../components/ui/button'
import { Sheet, SheetContent, SheetHeader, SheetTitle } from '../components/ui/sheet'
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem,
  DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger,
} from '../components/ui/dropdown-menu'
import {
  Collapsible, CollapsibleContent, CollapsibleTrigger,
} from '../components/ui/collapsible'
import { cn } from '../lib/utils'
import type { UserRole } from '../types/api'
import type { NavGroup, NavItem } from '../types/plugin'

function useGrantedFeatures(): Set<string> {
  const [features, setFeatures] = useState<Set<string>>(new Set())
  const { role } = usePermissions()
  useEffect(() => {
    if (!role) return
    apiClient
      .get<Array<{ plugin: string; feature: string; effective: boolean }>>('/feature-access/me')
      .then((r) => {
        setFeatures(new Set(
          r.data.filter((f) => f.effective).map((f) => `${f.plugin}:${f.feature}`)
        ))
      })
      .catch(() => {})
  }, [role])
  return features
}

export type { NavItem, NavGroup }

const ADMIN_ROLES: UserRole[] = ['admin', 'owner', 'moderator']

const ROLE_WEIGHT: Record<string, number> = { banned: 0, restricted: 1, user: 2, moderator: 3, admin: 4, owner: 5 }

function roleGte(role: string | null, minRoles: string | string[]): boolean {
  if (!role) return false
  const mins = Array.isArray(minRoles) ? minRoles : [minRoles]
  const w = ROLE_WEIGHT[role] ?? -1
  return mins.some((r) => w >= (ROLE_WEIGHT[r] ?? 99))
}

function canSeeItem(item: NavItem, role: string | null, grantedFeatures: Set<string>): boolean {
  if (item.minRole && !roleGte(role, item.minRole)) return false
  if (item.requiredFeature && !grantedFeatures.has(item.requiredFeature)) return false
  return true
}

function canSeeGroup(group: NavGroup, role: string | null, grantedFeatures: Set<string>): boolean {
  if (group.minRole && !roleGte(role, group.minRole)) return false
  return group.items.some((i) => canSeeItem(i, role, grantedFeatures))
}

const FLAVORS: { value: CatppuccinFlavor; label: string; color: string }[] = [
  { value: 'mocha',     label: 'Mocha',     color: '#cba6f7' },
  { value: 'macchiato', label: 'Macchiato', color: '#c6a0f6' },
  { value: 'frappe',    label: 'Frappe',    color: '#ca9ee6' },
  { value: 'latte',     label: 'Latte',     color: '#8839ef' },
]

function ThemePicker() {
  const { flavor, setFlavor } = useTelegram()
  const [dark, setDark] = useState(() => document.documentElement.classList.contains('dark'))
  const toggleDark = () => {
    const next = !dark
    setDark(next)
    document.documentElement.classList.toggle('dark', next)
  }
  return (
    <div className="flex items-center gap-1">
      <Button
        variant="ghost" size="icon" className="h-7 w-7"
        onClick={toggleDark}
      >
        {dark ? <Sun className="h-3.5 w-3.5" /> : <Moon className="h-3.5 w-3.5" />}
      </Button>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="icon" className="h-7 w-7">
            <Palette className="h-3.5 w-3.5" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-40">
          <DropdownMenuLabel className="text-xs text-muted-foreground">Flavor</DropdownMenuLabel>
          <DropdownMenuSeparator />
          {FLAVORS.map((f) => (
            <DropdownMenuItem
              key={f.value}
              onClick={() => setFlavor(f.value)}
              className={cn('gap-2', flavor === f.value && 'text-primary font-medium')}
            >
              <span className="h-3 w-3 shrink-0 rounded-full border border-border" style={{ backgroundColor: f.color }} />
              {f.label}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  )
}

function NavGroupSection({ group, role, grantedFeatures, currentPath, collapsed }: {
  group: NavGroup
  role: string | null
  grantedFeatures: Set<string>
  currentPath: string
  collapsed: boolean
}) {
  const { t } = useTranslation()
  const visible = group.items.filter((i) => canSeeItem(i, role, grantedFeatures))
  if (visible.length === 0) return null

  const getLabel = (item: NavItem) =>
    item.i18nKey ? t(item.i18nKey, { defaultValue: item.label }) : item.label

  const items = (
    <ul className="space-y-0.5">
      {visible.map((item) => {
        const isActive = currentPath === item.path || currentPath.startsWith(item.path + '/')
        return (
          <li key={item.path}>
            <NavLink
              to={item.path}
              title={collapsed ? getLabel(item) : undefined}
              className={cn(
                'flex items-center gap-3 rounded-md px-2 py-2 text-sm font-medium transition-colors',
                isActive
                  ? 'bg-sidebar-accent text-sidebar-accent-foreground'
                  : 'text-sidebar-foreground/70 hover:bg-sidebar-accent/50 hover:text-sidebar-foreground',
                collapsed && 'justify-center px-2',
              )}
            >
              <span className="shrink-0 [&>svg]:h-4 [&>svg]:w-4">{item.icon}</span>
              {!collapsed && <span className="truncate">{getLabel(item)}</span>}
            </NavLink>
          </li>
        )
      })}
    </ul>
  )

  if (group.collapsible && !collapsed) {
    return (
      <Collapsible defaultOpen={group.defaultOpen ?? true}>
        <div className="px-2 mb-0.5">
          <CollapsibleTrigger className="flex w-full items-center gap-1 py-1 text-xs font-semibold uppercase tracking-wider text-sidebar-foreground/50 hover:text-sidebar-foreground transition-colors">
            {group.label}
            <ChevronDown className="ml-auto h-3 w-3 transition-transform data-[state=open]:rotate-180" />
          </CollapsibleTrigger>
        </div>
        <CollapsibleContent>{items}</CollapsibleContent>
      </Collapsible>
    )
  }

  return (
    <div>
      {group.label && !collapsed && (
        <p className="px-2 mb-0.5 text-xs font-semibold uppercase tracking-wider text-sidebar-foreground/50">
          {group.label}
        </p>
      )}
      {items}
    </div>
  )
}

function AdminDrawer({ open, onClose, items, currentPath }: {
  open: boolean
  onClose: () => void
  items: NavItem[]
  currentPath: string
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const go = (path: string) => { void navigate(path); onClose() }
  const getLabel = (item: NavItem) =>
    item.i18nKey ? t(item.i18nKey, { defaultValue: item.label }) : item.label
  return (
    <Sheet open={open} onOpenChange={(v) => !v && onClose()}>
      <SheetContent side="bottom" className="rounded-t-xl pb-8">
        <SheetHeader className="mb-4">
          <SheetTitle>{t('nav.admin', { defaultValue: 'Admin' })}</SheetTitle>
        </SheetHeader>
        <div className="grid grid-cols-2 gap-2">
          {items.map((item) => (
            <Button
              key={item.path}
              variant={currentPath.startsWith(item.path) ? 'default' : 'outline'}
              className="h-auto flex-col gap-2 py-4"
              onClick={() => go(item.path)}
            >
              {item.icon}
              <span className="text-xs font-medium">{getLabel(item)}</span>
            </Button>
          ))}
        </div>
      </SheetContent>
    </Sheet>
  )
}

interface AppLayoutProps {
  navGroups: NavGroup[]
  appName?: string
  userStatsEndpoint?: string
}

export function AppLayout({ navGroups, appName = 'Yoink', userStatsEndpoint }: AppLayoutProps) {
  const { t } = useTranslation()
  const { role } = usePermissions()
  const grantedFeatures = useGrantedFeatures()
  const location = useLocation()
  const { isTelegramApp } = useTelegram()

  const [collapsed, setCollapsed] = useState(false)
  const [adminDrawerOpen, setAdminDrawerOpen] = useState(false)

  const getLabel = (item: NavItem) =>
    item.i18nKey ? t(item.i18nKey, { defaultValue: item.label }) : item.label

  const visibleGroups = navGroups.filter((g) => canSeeGroup(g, role ?? null, grantedFeatures))

  const regularItems = visibleGroups
    .filter((g) => !g.minRole)
    .flatMap((g) => g.items.filter((i) => canSeeItem(i, role ?? null, grantedFeatures)))

  const adminItems = visibleGroups
    .filter((g) => g.minRole)
    .flatMap((g) => g.items.filter((i) => canSeeItem(i, role ?? null, grantedFeatures)))

  const isAdmin = role && ADMIN_ROLES.includes(role as UserRole)
  const isAdminPath = adminItems.some((i) => location.pathname.startsWith(i.path))

  const currentItem = visibleGroups.flatMap((g) => g.items).find((i) =>
    location.pathname === i.path || location.pathname.startsWith(i.path + '/')
  )
  const pageTitle = currentItem ? getLabel(currentItem) : appName

  const sidebarWidth = collapsed ? 'w-14' : 'w-56'

  return (
    <div className="flex min-h-screen bg-background">

      {/* Desktop sidebar */}
      <aside className={cn(
        'hidden md:flex flex-col fixed inset-y-0 left-0 z-30 border-r bg-sidebar transition-[width] duration-200 ease-in-out',
        sidebarWidth,
      )}>
        {/* Sidebar header */}
        <div className="flex h-12 shrink-0 items-center border-b px-2 gap-2">
          {!collapsed && (
            <span className="flex-1 truncate font-bold text-sm px-1">{appName}</span>
          )}
          {!isTelegramApp && !collapsed && <ThemePicker />}
          <Button
            variant="ghost" size="icon" className="h-7 w-7 shrink-0"
            onClick={() => setCollapsed((c) => !c)}
          >
            {collapsed ? <Menu className="h-4 w-4" /> : <X className="h-4 w-4" />}
          </Button>
        </div>

        {/* Nav */}
        <nav className="flex-1 overflow-y-auto overflow-x-hidden p-2 space-y-4">
          {visibleGroups.map((group, i) => (
            <NavGroupSection
              key={group.label ?? i}
              group={group}
              role={role ?? null}
              grantedFeatures={grantedFeatures}
              currentPath={location.pathname}
              collapsed={collapsed}
            />
          ))}
        </nav>

        {/* Footer */}
        {!collapsed && (
          <div className="shrink-0 border-t p-2">
            <UserPanel statsEndpoint={userStatsEndpoint} />
          </div>
        )}
      </aside>

      {/* Main area */}
      <div className={cn(
        'flex flex-1 flex-col min-h-screen transition-[margin] duration-200 ease-in-out',
        collapsed ? 'md:ml-14' : 'md:ml-56',
      )}>
        {/* Header */}
        <header className="sticky top-0 z-20 flex h-12 shrink-0 items-center gap-2 border-b bg-background px-4">
          <span className="text-sm font-medium truncate">{pageTitle}</span>
        </header>

        {/* Content */}
        <div className="flex-1 overflow-y-auto pb-16 md:pb-0">
          <div className="mx-auto max-w-4xl px-4 py-6">
            <Outlet />
          </div>
        </div>
      </div>

      {/* Mobile bottom nav */}
      <nav className="fixed bottom-0 left-0 right-0 z-40 flex border-t bg-background md:hidden">
        {regularItems.map((item) => {
          const isActive = location.pathname === item.path || location.pathname.startsWith(item.path + '/')
          return (
            <NavLink
              key={item.path}
              to={item.path}
              className={cn(
                'flex flex-1 flex-col items-center justify-center gap-1 py-3 text-xs transition-colors',
                isActive ? 'text-primary' : 'text-muted-foreground hover:text-foreground',
              )}
            >
              <span className="[&>svg]:h-5 [&>svg]:w-5">{item.icon}</span>
              <span className="leading-none">{getLabel(item)}</span>
            </NavLink>
          )
        })}
        {isAdmin && adminItems.length > 0 && (
          <Button
            variant="ghost"
            onClick={() => setAdminDrawerOpen(true)}
            className={cn(
              'flex flex-1 flex-col items-center justify-center gap-1 py-2 h-auto rounded-none text-xs transition-colors',
              isAdminPath ? 'text-primary' : 'text-muted-foreground hover:text-foreground',
            )}
          >
            <Shield className="h-5 w-5" />
            <span className="leading-none">Admin</span>
          </Button>
        )}
      </nav>

      {isAdmin && (
        <AdminDrawer
          open={adminDrawerOpen}
          onClose={() => setAdminDrawerOpen(false)}
          items={adminItems}
          currentPath={location.pathname}
        />
      )}
    </div>
  )
}
