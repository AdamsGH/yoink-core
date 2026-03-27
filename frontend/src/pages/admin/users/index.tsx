import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { apiClient } from '@core/lib/api-client'
import { formatDate } from '@core/lib/utils'
import type { Feature, Permission, User, UserRole, UserUpdateRequest } from '@core/types/api'
import { Badge, type BadgeProps } from '@core/components/ui/badge'
import { Button } from '@core/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@core/components/ui/card'
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@core/components/ui/dialog'
import { Input } from '@core/components/ui/input'
import { Label } from '@core/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@core/components/ui/select'
import { Sheet, SheetContent, SheetHeader, SheetTitle } from '@core/components/ui/sheet'
import { Skeleton } from '@core/components/ui/skeleton'
import { Switch } from '@core/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@core/components/ui/table'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@core/components/ui/tabs'
import { toast } from '@core/components/ui/toast'

const ROLES: UserRole[] = ['owner', 'admin', 'moderator', 'user', 'restricted', 'banned']
const PAGE_SIZE = 30
type StatusFilter = 'all' | 'active' | 'restricted' | 'banned'

interface UserStats {
  total: number
  this_week: number
  today: number
  top_domains: { domain: string; count: number }[]
  member_since: string
}

export function roleBadgeVariant(role: UserRole): BadgeProps['variant'] {
  if (role === 'owner') return 'default'
  if (role === 'admin') return 'secondary'
  if (role === 'moderator') return 'outline'
  if (role === 'banned') return 'destructive'
  if (role === 'restricted') return 'warning'
  return 'outline'
}

function StatCell({ label, value }: { label: string; value: string | number }) {
  return (
    <div className="rounded-lg bg-muted/50 p-3 text-center">
      <div className="text-xl font-bold">{value}</div>
      <div className="text-xs text-muted-foreground mt-0.5">{label}</div>
    </div>
  )
}

function UserSheet({
  user,
  onClose,
  onUpdated,
}: {
  user: User | null
  onClose: () => void
  onUpdated: (u: User) => void
}) {
  const { t } = useTranslation()

  const [stats, setStats] = useState<UserStats | null>(null)
  const [statsLoading, setStatsLoading] = useState(false)

  const [features, setFeatures] = useState<Feature[]>([])
  const [permissions, setPermissions] = useState<Permission[]>([])
  const [permsLoading, setPermsLoading] = useState(false)
  const [togglingId, setTogglingId] = useState<string | null>(null)

  const [editRole, setEditRole] = useState<UserRole>('user')
  const [banUntil, setBanUntil] = useState('')
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    if (!user) { setStats(null); setPermissions([]); return }

    setEditRole(user.role)
    setBanUntil(user.ban_until ? user.ban_until.slice(0, 16) : '')

    setStatsLoading(true)
    apiClient
      .get<UserStats>(`/users/${user.id}/stats`)
      .then((r) => setStats(r.data))
      .catch(() => {})
      .finally(() => setStatsLoading(false))

    setPermsLoading(true)
    Promise.all([
      apiClient.get<Feature[]>('/features'),
      apiClient.get<Permission[]>(`/users/${user.id}/permissions`),
    ])
      .then(([fRes, pRes]) => {
        setFeatures(fRes.data)
        setPermissions(pRes.data)
      })
      .catch(() => {})
      .finally(() => setPermsLoading(false))
  }, [user?.id])

  const hasPermission = (plugin: string, feature: string) =>
    permissions.some((p) => p.plugin === plugin && p.feature === feature)

  const togglePermission = async (plugin: string, feature: string, grant: boolean) => {
    if (!user) return
    const key = `${plugin}.${feature}`
    setTogglingId(key)
    try {
      if (grant) {
        const res = await apiClient.post<Permission>(`/users/${user.id}/permissions`, { plugin, feature })
        setPermissions((prev) => [...prev, res.data])
        toast.success(t('permissions.granted'))
      } else {
        await apiClient.delete(`/users/${user.id}/permissions/${plugin}/${feature}`)
        setPermissions((prev) => prev.filter((p) => !(p.plugin === plugin && p.feature === feature)))
        toast.success(t('permissions.revoked'))
      }
    } catch {
      toast.error(grant ? t('permissions.grant_error') : t('permissions.revoke_error'))
    } finally {
      setTogglingId(null)
    }
  }

  const saveUser = async () => {
    if (!user) return
    setSaving(true)
    try {
      const body: UserUpdateRequest = {}
      if (editRole !== user.role) body.role = editRole
      if (banUntil) body.ban_until = new Date(banUntil).toISOString()
      else if (user.ban_until) body.ban_until = null
      const res = await apiClient.patch<User>(`/users/${user.id}`, body)
      toast.success(t('users.update_success'))
      onUpdated(res.data)
    } catch {
      toast.error(t('users.update_error'))
    } finally {
      setSaving(false)
    }
  }

  const groupedFeatures = features.reduce<Record<string, Feature[]>>((acc, f) => {
    (acc[f.plugin] ??= []).push(f)
    return acc
  }, {})

  return (
    <Sheet open={!!user} onOpenChange={(o) => !o && onClose()}>
      <SheetContent className="w-full sm:max-w-md flex flex-col p-0">
        {user && (
          <>
            <SheetHeader className="px-4 pt-4 pb-2 border-b shrink-0">
              <SheetTitle className="flex items-center gap-2 flex-wrap">
                <span>{user.first_name ?? String(user.id)}</span>
                <Badge variant={roleBadgeVariant(user.role)}>{user.role}</Badge>
              </SheetTitle>
              <div className="flex items-center gap-3 text-xs text-muted-foreground">
                {user.username && <span>@{user.username}</span>}
                <span className="font-mono">#{user.id}</span>
              </div>
            </SheetHeader>

            <Tabs defaultValue="stats" className="flex flex-col flex-1 min-h-0">
              <TabsList className="mx-4 mt-3 shrink-0">
                <TabsTrigger value="stats" className="flex-1">{t('users.tab_stats')}</TabsTrigger>
                <TabsTrigger value="access" className="flex-1">{t('users.tab_access')}</TabsTrigger>
                <TabsTrigger value="edit" className="flex-1">{t('users.tab_edit')}</TabsTrigger>
              </TabsList>

              <div className="flex-1 overflow-y-auto">
                <TabsContent value="stats" className="px-4 py-3 space-y-4 mt-0">
                  {statsLoading ? (
                    <div className="grid grid-cols-3 gap-2">
                      {[0, 1, 2].map((i) => <Skeleton key={i} className="h-16 rounded-lg" />)}
                    </div>
                  ) : stats ? (
                    <>
                      <div className="grid grid-cols-3 gap-2">
                        <StatCell label={t('users.total')} value={stats.total.toLocaleString()} />
                        <StatCell label={t('users.this_week')} value={stats.this_week.toLocaleString()} />
                        <StatCell label={t('users.today')} value={stats.today.toLocaleString()} />
                      </div>
                      {stats.top_domains.length > 0 && (
                        <div className="space-y-1.5">
                          <p className="text-xs text-muted-foreground">{t('users.top_domains')}</p>
                          <div className="space-y-1">
                            {stats.top_domains.map((d) => (
                              <div key={d.domain} className="flex items-center justify-between text-sm">
                                <span className="font-mono text-xs truncate">{d.domain}</span>
                                <span className="text-muted-foreground ml-2 shrink-0">{d.count}</span>
                              </div>
                            ))}
                          </div>
                        </div>
                      )}
                      {stats.total === 0 && (
                        <p className="text-sm text-muted-foreground text-center py-4">{t('users.no_downloads')}</p>
                      )}
                      <div className="text-xs text-muted-foreground border-t pt-3 space-y-1">
                        <div className="flex justify-between">
                          <span>{t('users.col_joined')}</span>
                          <span>{formatDate(user.created_at)}</span>
                        </div>
                        <div className="flex justify-between">
                          <span>{t('users.last_seen')}</span>
                          <span>{formatDate(user.updated_at)}</span>
                        </div>
                      </div>
                    </>
                  ) : (
                    <p className="text-sm text-muted-foreground text-center py-4">{t('common.load_error')}</p>
                  )}
                </TabsContent>

                <TabsContent value="access" className="px-4 py-3 space-y-4 mt-0">
                  {permsLoading ? (
                    <div className="space-y-3">
                      {[0, 1].map((i) => <Skeleton key={i} className="h-12 w-full rounded-md" />)}
                    </div>
                  ) : features.length === 0 ? (
                    <p className="text-sm text-muted-foreground text-center py-4">{t('permissions.no_features')}</p>
                  ) : (
                    Object.entries(groupedFeatures).map(([plugin, feats]) => (
                      <div key={plugin} className="space-y-2">
                        <p className="text-xs font-medium text-muted-foreground uppercase tracking-wide">{plugin}</p>
                        <div className="rounded-md border divide-y">
                          {feats.map((f) => {
                            const key = `${f.plugin}.${f.feature}`
                            const active = hasPermission(f.plugin, f.feature)
                            return (
                              <div key={key} className="flex items-center justify-between px-3 py-2.5 gap-3">
                                <div className="min-w-0">
                                  <p className="text-sm font-medium">{f.label}</p>
                                  {f.description && (
                                    <p className="text-xs text-muted-foreground truncate">{f.description}</p>
                                  )}
                                </div>
                                <Switch
                                  checked={active}
                                  disabled={togglingId === key}
                                  onCheckedChange={(v) => void togglePermission(f.plugin, f.feature, v)}
                                />
                              </div>
                            )
                          })}
                        </div>
                      </div>
                    ))
                  )}
                </TabsContent>

                <TabsContent value="edit" className="px-4 py-3 space-y-4 mt-0">
                  <div className="space-y-1.5">
                    <Label>{t('users.col_role')}</Label>
                    <Select value={editRole} onValueChange={(v) => setEditRole(v as UserRole)}>
                      <SelectTrigger><SelectValue /></SelectTrigger>
                      <SelectContent>
                        {ROLES.filter((r) => r !== 'owner').map((r) => (
                          <SelectItem key={r} value={r}>{r}</SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                  <div className="space-y-1.5">
                    <Label htmlFor="ban-until">{t('users.ban_until_label')}</Label>
                    <Input
                      id="ban-until"
                      type="datetime-local"
                      value={banUntil}
                      onChange={(e) => setBanUntil(e.target.value)}
                    />
                    <p className="text-xs text-muted-foreground">{t('users.ban_until_hint')}</p>
                  </div>
                  <Button className="w-full" onClick={saveUser} disabled={saving}>
                    {saving ? t('common.saving') : t('common.save')}
                  </Button>
                </TabsContent>
              </div>
            </Tabs>
          </>
        )}
      </SheetContent>
    </Sheet>
  )
}

export default function AdminUsersPage() {
  const { t } = useTranslation()

  const [items, setItems] = useState<User[]>([])
  const [total, setTotal] = useState(0)
  const [page, setPage] = useState(1)
  const [loading, setLoading] = useState(true)

  const [search, setSearch] = useState('')
  const [draftSearch, setDraftSearch] = useState('')
  const [filters, setFilters] = useState<{ role: string; status: StatusFilter }>({ role: 'all', status: 'all' })
  const [draftFilters, setDraftFilters] = useState(filters)
  const [showFilters, setShowFilters] = useState(false)

  const [viewed, setViewed] = useState<User | null>(null)

  const hasActive = search !== '' || filters.role !== 'all' || filters.status !== 'all'
  const totalPages = Math.max(1, Math.ceil(total / PAGE_SIZE))

  const load = async (p = page) => {
    setLoading(true)
    try {
      const params: Record<string, string | number> = { offset: (p - 1) * PAGE_SIZE, limit: PAGE_SIZE }
      if (search) params.search = search
      if (filters.role !== 'all') params.role = filters.role
      if (filters.status !== 'all') params.status = filters.status
      const res = await apiClient.get<{ items: User[]; total: number }>('/users', { params })
      setItems(res.data.items)
      setTotal(res.data.total)
    } catch {
      toast.error(t('common.load_error'))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => { void load(page) }, [page, search, filters])

  const apply = () => {
    setSearch(draftSearch)
    setFilters(draftFilters)
    setPage(1)
    setShowFilters(false)
  }

  const resetFilters = () => {
    const empty = { role: 'all', status: 'all' as StatusFilter }
    setDraftSearch(''); setSearch('')
    setDraftFilters(empty); setFilters(empty)
    setPage(1)
    setShowFilters(false)
  }

  const quickBan = async (u: User) => {
    try {
      await apiClient.patch(`/users/${u.id}`, { role: 'banned' } as UserUpdateRequest)
      toast.success(t('users.banned'))
      void load(page)
    } catch { toast.error(t('users.update_error')) }
  }

  const quickUnban = async (u: User) => {
    try {
      await apiClient.patch(`/users/${u.id}`, { role: 'user' } as UserUpdateRequest)
      toast.success(t('users.unbanned'))
      void load(page)
    } catch { toast.error(t('users.update_error')) }
  }

  const handleUpdated = (updated: User) => {
    setViewed(updated)
    setItems((prev) => prev.map((u) => u.id === updated.id ? updated : u))
  }

  return (
    <div className="space-y-3 p-4">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold">{t('users.title')}</h1>
        <Button variant="outline" size="sm" onClick={() => setShowFilters((v) => !v)}>
          {t('users.filter_toggle')}
          {hasActive && <span className="ml-1.5 h-1.5 w-1.5 rounded-full bg-primary inline-block" />}
        </Button>
      </div>

      {showFilters && (
        <Card>
          <CardContent className="pt-3 pb-3 space-y-3">
            <div className="space-y-1">
              <Label className="text-xs text-muted-foreground">{t('common.search')}</Label>
              <Input
                placeholder={t('users.search_placeholder')}
                value={draftSearch}
                onChange={(e) => setDraftSearch(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && apply()}
              />
            </div>
            <div className="grid grid-cols-2 gap-2">
              <div className="space-y-1">
                <Label className="text-xs text-muted-foreground">{t('users.filter_role')}</Label>
                <Select value={draftFilters.role} onValueChange={(v) => setDraftFilters((f) => ({ ...f, role: v }))}>
                  <SelectTrigger><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">{t('common.no_data')}</SelectItem>
                    {ROLES.map((r) => <SelectItem key={r} value={r}>{r}</SelectItem>)}
                  </SelectContent>
                </Select>
              </div>
              <div className="space-y-1">
                <Label className="text-xs text-muted-foreground">{t('users.filter_status')}</Label>
                <Select value={draftFilters.status} onValueChange={(v) => setDraftFilters((f) => ({ ...f, status: v as StatusFilter }))}>
                  <SelectTrigger><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">{t('common.no_data')}</SelectItem>
                    <SelectItem value="active">{t('users.filter_active')}</SelectItem>
                    <SelectItem value="restricted">{t('users.filter_restricted')}</SelectItem>
                    <SelectItem value="banned">{t('users.filter_banned')}</SelectItem>
                  </SelectContent>
                </Select>
              </div>
            </div>
            <div className="flex gap-2">
              <Button size="sm" onClick={apply}>{t('users.filter_apply')}</Button>
              {hasActive && <Button size="sm" variant="outline" onClick={resetFilters}>{t('users.filter_clear')}</Button>}
            </div>
          </CardContent>
        </Card>
      )}

      <Card>
        <CardHeader className="py-2 px-4">
          <CardTitle className="text-sm font-normal text-muted-foreground">
            {total.toLocaleString()} {t('users.count')}
            {hasActive && <span className="ml-2 text-xs">(filtered)</span>}
          </CardTitle>
        </CardHeader>
        <CardContent className="p-0">
          {loading ? (
            <div className="flex justify-center py-10 text-muted-foreground">
              <div className="h-5 w-5 animate-spin rounded-full border-2 border-border border-t-primary" />
            </div>
          ) : items.length === 0 ? (
            <div className="flex justify-center py-10 text-sm text-muted-foreground">{t('users.no_users')}</div>
          ) : (
            <>
              <div className="hidden md:block overflow-x-auto">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>{t('users.col_user')}</TableHead>
                      <TableHead>{t('users.col_id')}</TableHead>
                      <TableHead>{t('users.col_role')}</TableHead>
                      <TableHead>{t('users.col_joined')}</TableHead>
                      <TableHead />
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {items.map((user) => (
                      <TableRow
                        key={user.id}
                        className="cursor-pointer hover:bg-muted/50"
                        onClick={() => setViewed(user)}
                      >
                        <TableCell>
                          <p className="text-sm font-medium">{user.first_name ?? user.username ?? '-'}</p>
                          {user.username && <p className="text-xs text-muted-foreground">@{user.username}</p>}
                        </TableCell>
                        <TableCell className="font-mono text-xs">{user.id}</TableCell>
                        <TableCell><Badge variant={roleBadgeVariant(user.role)}>{user.role}</Badge></TableCell>
                        <TableCell className="text-xs text-muted-foreground">{formatDate(user.created_at)}</TableCell>
                        <TableCell onClick={(e) => e.stopPropagation()}>
                          <div className="flex gap-1">
                            {user.role === 'banned' ? (
                              <Button variant="ghost" size="sm" className="text-green-600 hover:text-green-700" onClick={() => void quickUnban(user)}>{t('users.quick_unban')}</Button>
                            ) : user.role !== 'owner' ? (
                              <Button variant="ghost" size="sm" className="text-destructive hover:text-destructive" onClick={() => void quickBan(user)}>{t('users.quick_ban')}</Button>
                            ) : null}
                          </div>
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </div>

              <div className="md:hidden divide-y divide-border">
                {items.map((user) => (
                  <div
                    key={user.id}
                    className="px-4 py-3 space-y-2 cursor-pointer active:bg-muted/50"
                    onClick={() => setViewed(user)}
                  >
                    <div className="flex items-start justify-between gap-2">
                      <div className="min-w-0">
                        <p className="text-sm font-medium truncate">{user.first_name ?? user.username ?? '-'}</p>
                        <p className="text-xs text-muted-foreground font-mono">{user.id}{user.username && ` · @${user.username}`}</p>
                      </div>
                      <Badge variant={roleBadgeVariant(user.role)} className="shrink-0">{user.role}</Badge>
                    </div>
                    <div className="flex gap-2" onClick={(e) => e.stopPropagation()}>
                      {user.role === 'banned' ? (
                        <Button variant="outline" size="sm" className="flex-1 text-green-600 border-green-600/30" onClick={() => void quickUnban(user)}>{t('users.quick_unban')}</Button>
                      ) : user.role !== 'owner' ? (
                        <Button variant="outline" size="sm" className="flex-1 text-destructive border-destructive/30" onClick={() => void quickBan(user)}>{t('users.quick_ban')}</Button>
                      ) : null}
                    </div>
                  </div>
                ))}
              </div>
            </>
          )}
        </CardContent>
      </Card>

      {totalPages > 1 && (
        <div className="flex items-center justify-between">
          <span className="text-sm text-muted-foreground">{t('users.page_of', { page, total: totalPages })}</span>
          <div className="flex gap-2">
            <Button variant="outline" size="sm" disabled={page === 1} onClick={() => setPage((p) => p - 1)}>{t('users.prev')}</Button>
            <Button variant="outline" size="sm" disabled={page === totalPages} onClick={() => setPage((p) => p + 1)}>{t('users.next')}</Button>
          </div>
        </div>
      )}

      <UserSheet user={viewed} onClose={() => setViewed(null)} onUpdated={handleUpdated} />
    </div>
  )
}
