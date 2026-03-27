import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { NavLink, Outlet, useLocation, useNavigate } from 'react-router'
import { ChevronDown, Palette, Shield } from 'lucide-react'

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
  Sidebar, SidebarContent, SidebarFooter, SidebarGroup,
  SidebarGroupContent, SidebarGroupLabel, SidebarHeader,
  SidebarMenu, SidebarMenuButton, SidebarMenuItem,
  SidebarInset, SidebarProvider, SidebarRail, SidebarTrigger,
} from '../components/ui/sidebar'
import {
  Collapsible, CollapsibleContent, CollapsibleTrigger,
} from '../components/ui/collapsible'
import { cn } from '../lib/utils'
import type { UserRole } from '../types/api'
import type { NavGroup, NavItem } from '../types/plugin'

export type { NavItem, NavGroup }

interface AppLayoutProps {
  navGroups: NavGroup[]
  appName?: string
  userStatsEndpoint?: string
}

const ROLE_WEIGHT: Record<string, number> = {
  owner: 0, admin: 1, moderator: 2, user: 3, restricted: 4, banned: 5,
}
const ADMIN_ROLES: UserRole[] = ['owner', 'admin', 'moderator']

function hasRole(role: string | null, minRole: string[]): boolean {
  if (!role) return false
  const w = ROLE_WEIGHT[role] ?? 99
  return minRole.some((r) => w <= (ROLE_WEIGHT[r] ?? 99))
}

function canSeeItem(item: NavItem, role: string | null, grantedFeatures: Set<string>): boolean {
  if (item.minRole && !hasRole(role, item.minRole)) return false
  if (item.requiredFeature && !grantedFeatures.has(item.requiredFeature)) return false
  return true
}

function canSeeGroup(group: NavGroup, role: string | null, grantedFeatures: Set<string>): boolean {
  if (group.minRole && !hasRole(role, Array.isArray(group.minRole) ? group.minRole : [group.minRole])) return false
  return group.items.some((i) => canSeeItem(i, role, grantedFeatures))
}

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

const FLAVORS: { value: CatppuccinFlavor; label: string; color: string }[] = [
  { value: 'latte',     label: 'Latte',     color: '#df8e1d' },
  { value: 'frappe',    label: 'Frappé',    color: '#85c1dc' },
  { value: 'macchiato', label: 'Macchiato', color: '#c6a0f6' },
  { value: 'mocha',     label: 'Mocha',     color: '#f38ba8' },
]

function ThemePicker() {
  const { flavor, setFlavor } = useTelegram()
  return (
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
  )
}

function SidebarNavGroup({ group, role, grantedFeatures, currentPath }: {
  group: NavGroup
  role: string | null
  grantedFeatures: Set<string>
  currentPath: string
}) {
  const { t } = useTranslation()
  const visible = group.items.filter((i) => canSeeItem(i, role, grantedFeatures))
  if (visible.length === 0) return null

  const getLabel = (item: NavItem) =>
    item.i18nKey ? t(item.i18nKey, { defaultValue: item.label }) : item.label

  const items = (
    <SidebarMenu>
      {visible.map((item) => (
        <SidebarMenuItem key={item.path}>
          <SidebarMenuButton
            asChild
            isActive={currentPath === item.path || currentPath.startsWith(item.path + '/')}
            tooltip={getLabel(item)}
          >
            <NavLink to={item.path}>
              {item.icon}
              <span>{getLabel(item)}</span>
            </NavLink>
          </SidebarMenuButton>
        </SidebarMenuItem>
      ))}
    </SidebarMenu>
  )

  if (group.collapsible) {
    return (
      <Collapsible defaultOpen={group.defaultOpen ?? true} className="group/collapsible">
        <SidebarGroup className="p-0">
          <SidebarGroupLabel asChild>
            <CollapsibleTrigger className="flex w-full items-center">
              {group.label}
              <ChevronDown className="ml-auto h-4 w-4 transition-transform group-data-[state=open]/collapsible:rotate-180" />
            </CollapsibleTrigger>
          </SidebarGroupLabel>
          <CollapsibleContent>
            <SidebarGroupContent>{items}</SidebarGroupContent>
          </CollapsibleContent>
        </SidebarGroup>
      </Collapsible>
    )
  }

  return (
    <SidebarGroup>
      {group.label && <SidebarGroupLabel>{group.label}</SidebarGroupLabel>}
      <SidebarGroupContent>{items}</SidebarGroupContent>
    </SidebarGroup>
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

export function AppLayout({ navGroups, appName = 'Yoink', userStatsEndpoint }: AppLayoutProps) {
  const { t } = useTranslation()
  const { role } = usePermissions()
  const grantedFeatures = useGrantedFeatures()
  const getLabel = (item: NavItem) =>
    item.i18nKey ? t(item.i18nKey, { defaultValue: item.label }) : item.label
  const location = useLocation()
  const { isTelegramApp } = useTelegram()
  const [adminDrawerOpen, setAdminDrawerOpen] = useState(false)

  const visibleGroups = navGroups.filter((g) => canSeeGroup(g, role ?? null, grantedFeatures))

  const regularItems = visibleGroups
    .filter((g) => !g.minRole)
    .flatMap((g) => g.items.filter((i) => canSeeItem(i, role ?? null, grantedFeatures)))

  const adminItems = visibleGroups
    .filter((g) => g.minRole)
    .flatMap((g) => g.items.filter((i) => canSeeItem(i, role ?? null, grantedFeatures)))

  const isAdmin = role && ADMIN_ROLES.includes(role as UserRole)
  const isAdminPath = adminItems.some((i) => location.pathname.startsWith(i.path))

  return (
    <>
      {/* Desktop sidebar — standard shadcn layout, no extra className on Sidebar */}
      <SidebarProvider>
        <Sidebar collapsible="icon">
          <SidebarHeader>
            <div className="flex h-10 items-center gap-2 px-2">
              <SidebarTrigger className="-ml-1" />
              <span className="font-bold text-sm truncate flex-1 group-data-[collapsible=icon]:hidden">
                {appName}
              </span>
              {!isTelegramApp && (
                <span className="group-data-[collapsible=icon]:hidden">
                  <ThemePicker />
                </span>
              )}
            </div>
          </SidebarHeader>

          <SidebarContent>
            {visibleGroups.map((group, i) => (
              <SidebarNavGroup
                key={group.label ?? i}
                group={group}
                role={role ?? null}
                grantedFeatures={grantedFeatures}
                currentPath={location.pathname}
              />
            ))}
          </SidebarContent>

          <SidebarFooter className="group-data-[collapsible=icon]:hidden">
            <UserPanel statsEndpoint={userStatsEndpoint} />
          </SidebarFooter>

          <SidebarRail />
        </Sidebar>

        <SidebarInset>
          <header className="flex h-12 shrink-0 items-center gap-2 border-b px-4">
            <span className="text-sm font-medium">
              {(() => {
                const item = visibleGroups.flatMap((g) => g.items).find((i) =>
                  location.pathname === i.path || location.pathname.startsWith(i.path + '/')
                )
                return item ? getLabel(item) : appName
              })()}
            </span>
          </header>
          <div className="flex-1 overflow-y-auto pb-16 md:pb-0">
            <div className="mx-auto max-w-4xl px-4 py-6">
              <Outlet />
            </div>
          </div>
        </SidebarInset>
      </SidebarProvider>

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
    </>
  )
}
