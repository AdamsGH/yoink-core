import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { apiClient } from '@core/lib/api-client'
import { formatDate } from '@core/lib/utils'
import type { PaginatedResponse, User, UserRole, UserUpdateRequest } from '@core/types/api'
import { Badge, type BadgeProps } from '@core/components/ui/badge'
import { Button } from '@core/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@core/components/ui/card'
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@core/components/ui/dialog'
import { Input } from '@core/components/ui/input'
import { Label } from '@core/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@core/components/ui/select'
import { Sheet, SheetContent, SheetHeader, SheetTitle } from '@core/components/ui/sheet'
import { Skeleton } from '@core/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@core/components/ui/table'
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

function roleBadgeVariant(role: UserRole): BadgeProps['variant'] {
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

function UserStatsSheet({
  user,
  onClose,
  onEdit,
}: {
  user: User | null
  onClose: () => void
  onEdit: (u: User) => void
}) {
  const { t } = useTranslation()
  const [stats, setStats] = useState<UserStats | null>(null)
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    if (!user) { setStats(null); return }
    setLoading(true)
    apiClient
      .get<UserStats>(`/users/${user.id}/stats`)
      .then((r) => setStats(r.data))
      .catch(() => toast.error(t('users.update_error')))
      .finally(() => setLoading(false))
  }, [user?.id])

  return (
    <Sheet open={!!user} onOpenChange={(o) => !o && onClose()}>
      <SheetContent className="w-full sm:max-w-md overflow-y-auto">
        {user && (
          <>
            <SheetHeader className="pb-4">
              <SheetTitle className="flex items-center gap-2">
                <span>{user.first_name ?? String(user.id)}</span>
                <Badge variant={roleBadgeVariant(user.role)} className="ml-1">{user.role}</Badge>
              </SheetTitle>
              {user.username && (
                <p className="text-sm text-muted-foreground">@{user.username}</p>
              )}
              <p className="text-xs text-muted-foreground font-mono">ID: {user.id}</p>
            </SheetHeader>

            <div className="space-y-5">
              <div className="space-y-2">
                <h3 className="text-sm font-medium text-muted-foreground">{t('users.account')}</h3>
                <div className="text-sm space-y-1">
                  <div className="flex justify-between">
                    <span className="text-muted-foreground">{t('users.col_joined')}</span>
                    <span>{formatDate(user.created_at)}</span>
                  </div>
                  <div className="flex justify-between">
                    <span className="text-muted-foreground">{t('users.last_seen')}</span>
                    <span>{formatDate(user.updated_at)}</span>
                  </div>
                </div>
              </div>

              <div className="space-y-2">
                <h3 className="text-sm font-medium text-muted-foreground">{t('users.downloads')}</h3>
                {loading ? (
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
                      <div className="space-y-1.5 pt-1">
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
                      <p className="text-sm text-muted-foreground text-center py-2">{t('users.no_downloads')}</p>
                    )}
                  </>
                ) : null}
              </div>

              <div className="flex gap-2 pt-2">
                <Button className="flex-1" onClick={() => { onClose(); onEdit(user) }}>
                  {t('users.edit_user_btn')}
                </Button>
              </div>
            </div>
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
  const [filters, setFilters] = useState({ search: '', role: 'all' as UserRole | 'all', status: 'all' as StatusFilter })
  const [activeFilters, setActiveFilters] = useState(filters)

  const [viewed, setViewed] = useState<User | null>(null)
  const [selected, setSelected] = useState<User | null>(null)
  const [editRole, setEditRole] = useState<UserRole>('user')
  const [banUntil, setBanUntil] = useState('')
  const [saving, setSaving] = useState(false)

  const totalPages = Math.ceil(total / PAGE_SIZE)
  const hasActive = activeFilters.search !== '' || activeFilters.role !== 'all' || activeFilters.status !== 'all'

  const load = (p = page, f = activeFilters) => {
    setLoading(true)
    const params: Record<string, string | number> = { offset: (p - 1) * PAGE_SIZE, limit: PAGE_SIZE }
    if (f.search) params.search = f.search
    if (f.role !== 'all') params.role = f.role
    if (f.status !== 'all') params.status = f.status
    apiClient
      .get<PaginatedResponse<User>>('/users', { params })
      .then((r) => { setItems(r.data.items); setTotal(r.data.total) })
      .catch(() => toast.error(t('common.load_error')))
      .finally(() => setLoading(false))
  }

  useEffect(() => { load() }, [page])

  const apply = () => {
    setActiveFilters(filters)
    setPage(1)
    load(1, filters)
  }

  const resetFilters = () => {
    const def = { search: '', role: 'all' as const, status: 'all' as const }
    setFilters(def)
    setActiveFilters(def)
    setPage(1)
    load(1, def)
  }

  const openEdit = (user: User) => {
    setSelected(user)
    setEditRole(user.role)
    setBanUntil('')
  }

  const saveUser = async () => {
    if (!selected) return
    setSaving(true)
    try {
      const body: UserUpdateRequest = { role: editRole }
      if (banUntil) body.ban_until = new Date(banUntil).toISOString()
      await apiClient.patch(`/users/${selected.id}`, body)
      toast.success(t('users.role_updated'))
      setSelected(null)
      load()
    } catch {
      toast.error(t('users.update_error'))
    } finally {
      setSaving(false)
    }
  }

  const quickBan = async (user: User) => {
    try {
      await apiClient.patch(`/users/${user.id}`, { role: 'banned' } as UserUpdateRequest)
      toast.success(`${user.first_name ?? user.id} banned`)
      load()
    } catch { toast.error(t('users.update_error')) }
  }

  const quickUnban = async (user: User) => {
    try {
      await apiClient.patch(`/users/${user.id}`, { role: 'user' } as UserUpdateRequest)
      toast.success(`${user.first_name ?? user.id} unbanned`)
      load()
    } catch { toast.error(t('users.update_error')) }
  }

  return (
    <div className="space-y-4">
      <h1 className="text-2xl font-bold">{t('users.title')}</h1>

      <Card>
        <CardContent className="pt-4">
          <div className="grid gap-3 sm:grid-cols-3">
            <div className="space-y-1.5">
              <Label className="text-xs text-muted-foreground">Search by username / ID</Label>
              <Input placeholder="@username or 12345678" value={filters.search}
                onChange={(e) => setFilters((f) => ({ ...f, search: e.target.value }))}
                onKeyDown={(e) => e.key === 'Enter' && apply()} />
            </div>
            <div className="space-y-1.5">
              <Label className="text-xs text-muted-foreground">{t('users.col_role')}</Label>
              <Select value={filters.role} onValueChange={(v: string) => setFilters((f) => ({ ...f, role: v as UserRole | 'all' }))}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">{t('users.filter_all_roles')}</SelectItem>
                  {ROLES.map((r) => <SelectItem key={r} value={r}>{r}</SelectItem>)}
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-1.5">
              <Label className="text-xs text-muted-foreground">{t('users.filter_status')}</Label>
              <Select value={filters.status} onValueChange={(v: string) => setFilters((f) => ({ ...f, status: v as StatusFilter }))}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">All</SelectItem>
                  <SelectItem value="active">{t('users.filter_active')}</SelectItem>
                  <SelectItem value="restricted">{t('users.filter_restricted')}</SelectItem>
                  <SelectItem value="banned">{t('users.filter_banned')}</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>
          <div className="mt-3 flex gap-2">
            <Button size="sm" onClick={apply}>{t('users.filter_apply')}</Button>
            {hasActive && <Button size="sm" variant="outline" onClick={resetFilters}>{t('users.filter_clear')}</Button>}
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>
            {total.toLocaleString()} user{total !== 1 ? 's' : ''}
            {hasActive && <span className="ml-2 text-sm font-normal text-muted-foreground">(filtered)</span>}
          </CardTitle>
        </CardHeader>
        <CardContent className="p-0">
          {loading ? (
            <div className="flex justify-center py-12 text-muted-foreground">Loading…</div>
          ) : items.length === 0 ? (
            <div className="flex justify-center py-12 text-muted-foreground">{t('users.no_users')}</div>
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
                            <Button variant="ghost" size="sm" onClick={() => openEdit(user)}>{t('common.edit')}</Button>
                            {user.role === 'banned' ? (
                              <Button variant="ghost" size="sm" className="text-green-600 hover:text-green-700" onClick={() => quickUnban(user)}>{t('users.quick_unban')}</Button>
                            ) : user.role !== 'owner' ? (
                              <Button variant="ghost" size="sm" className="text-destructive hover:text-destructive" onClick={() => quickBan(user)}>{t('users.quick_ban')}</Button>
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
                      <Button variant="outline" size="sm" className="flex-1" onClick={() => openEdit(user)}>{t('common.edit')}</Button>
                      {user.role === 'banned' ? (
                        <Button variant="outline" size="sm" className="flex-1 text-green-600 border-green-600/30" onClick={() => quickUnban(user)}>{t('users.quick_unban')}</Button>
                      ) : user.role !== 'owner' ? (
                        <Button variant="outline" size="sm" className="flex-1 text-destructive border-destructive/30" onClick={() => quickBan(user)}>{t('users.quick_ban')}</Button>
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

      <UserStatsSheet
        user={viewed}
        onClose={() => setViewed(null)}
        onEdit={(u) => { setViewed(null); openEdit(u) }}
      />

      <Dialog open={!!selected} onOpenChange={(open: boolean) => !open && setSelected(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t('users.edit_title', { id: selected?.id })}</DialogTitle>
          </DialogHeader>
          {selected && (
            <div className="space-y-4">
              <div>
                <p className="text-sm font-medium">{selected.first_name}</p>
                {selected.username && <p className="text-xs text-muted-foreground">@{selected.username}</p>}
              </div>
              <div className="space-y-1.5">
                <Label>{t('users.col_role')}</Label>
                <Select value={editRole} onValueChange={(v: string) => setEditRole(v as UserRole)}>
                  <SelectTrigger><SelectValue /></SelectTrigger>
                  <SelectContent>
                    {ROLES.map((r) => <SelectItem key={r} value={r}>{r}</SelectItem>)}
                  </SelectContent>
                </Select>
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="ban-until">{t('users.ban_until_label')}</Label>
                <Input id="ban-until" type="datetime-local" value={banUntil} onChange={(e) => setBanUntil(e.target.value)} />
                <p className="text-xs text-muted-foreground">{t('users.ban_until_hint')}</p>
              </div>
            </div>
          )}
          <DialogFooter>
            <Button variant="outline" onClick={() => setSelected(null)}>{t('common.cancel')}</Button>
            <Button onClick={saveUser} disabled={saving}>{saving ? t('common.loading') : t('common.save')}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
