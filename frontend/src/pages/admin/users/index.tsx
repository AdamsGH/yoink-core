import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { ArrowDownAZ, ArrowUpAZ, Ban, ShieldCheck, Users } from 'lucide-react'

import { apiClient } from '@core/lib/api-client'
import { cn } from '@core/lib/utils'
import { RING_BY_ROLE, userInitials, userPhotoUrl } from '@core/lib/user-utils'
import type { User, UserRole, UserUpdateRequest } from '@core/types/api'
import { Avatar, AvatarFallback, AvatarImage, Button, Card, CardContent, CardHeader, CardTitle, Input, Item, ItemActions, ItemContent, ItemDescription, ItemTitle, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Skeleton, Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@ui'
import { RoleBadge } from '@app'
import { toast } from '@core/components/ui/toast'
import { UserDrawer } from './UserDrawer'

type StatusFilter = 'all' | 'active' | 'restricted' | 'banned'

const ROLES: UserRole[] = ['owner', 'admin', 'moderator', 'user', 'restricted', 'banned']
const PAGE_SIZE = 25

export default function AdminUsersPage() {
  const { t } = useTranslation()

  const [items, setItems] = useState<User[]>([])
  const [total, setTotal] = useState(0)
  const [page, setPage] = useState(1)
  const [initialLoading, setInitialLoading] = useState(true)
  const [fetching, setFetching] = useState(false)

  const [search, setSearch] = useState('')
  const [debouncedSearch, setDebouncedSearch] = useState('')
  const [filters, setFilters] = useState<{ role: string; status: StatusFilter }>({ role: 'all', status: 'all' })
  const [showFilters, setShowFilters] = useState(false)
  const [sort, setSort] = useState<'created_at' | 'updated_at' | 'name' | 'role' | 'dl_count' | 'dl_last_at'>('created_at')
  const [sortDir, setSortDir] = useState<'asc' | 'desc'>('desc')

  const [viewed, setViewed] = useState<User | null>(null)

  const hasActive = debouncedSearch !== '' || filters.role !== 'all' || filters.status !== 'all'
  const totalPages = Math.max(1, Math.ceil(total / PAGE_SIZE))

  useEffect(() => {
    const id = setTimeout(() => {
      setDebouncedSearch(search)
      setPage(1)
    }, 300)
    return () => clearTimeout(id)
  }, [search])

  const load = async (p: number, q: string, f: typeof filters, s: typeof sort, d: typeof sortDir) => {
    setFetching(true)
    try {
      const params: Record<string, string | number> = { offset: (p - 1) * PAGE_SIZE, limit: PAGE_SIZE, sort: s, direction: d }
      if (q) params.search = q
      if (f.role !== 'all') params.role = f.role
      if (f.status !== 'all') params.status = f.status
      const res = await apiClient.get<{ items: User[]; total: number }>('/users', { params })
      setItems(res.data.items)
      setTotal(res.data.total)
    } catch {
      toast.error(t('common.load_error'))
    } finally {
      setFetching(false)
      setInitialLoading(false)
    }
  }

  useEffect(() => { void load(page, debouncedSearch, filters, sort, sortDir) }, [page, debouncedSearch, filters, sort, sortDir])

  const resetFilters = () => {
    setSearch(''); setDebouncedSearch('')
    setFilters({ role: 'all', status: 'all' })
    setPage(1)
  }

  const quickBan = async (u: User) => {
    try {
      await apiClient.patch(`/users/${u.id}`, { role: 'banned' } as UserUpdateRequest)
      toast.success(t('users.banned'))
      void load(page, debouncedSearch, filters, sort, sortDir)
    } catch { toast.error(t('users.update_error')) }
  }

  const quickUnban = async (u: User) => {
    try {
      await apiClient.patch(`/users/${u.id}`, { role: 'user' } as UserUpdateRequest)
      toast.success(t('users.unbanned'))
      void load(page, debouncedSearch, filters, sort, sortDir)
    } catch { toast.error(t('users.update_error')) }
  }

  const handleUpdated = (updated: User) => {
    setViewed(updated)
    setItems((prev) => prev.map((u) => u.id === updated.id ? updated : u))
  }

  return (
    <TooltipProvider delayDuration={300}>
      <div className="space-y-3">
        <Card>
          <CardHeader className="px-4 py-3">
            <div className="flex items-center justify-between gap-2">
              <CardTitle className="flex items-center gap-2 text-base">
                <Users className="h-4 w-4 text-muted-foreground" />
                {initialLoading
                  ? t('users.title', { defaultValue: 'Users' })
                  : `${total.toLocaleString()} ${t('users.count')}`}
              </CardTitle>
              <Button variant="ghost" size="sm" className="h-7 px-2 text-xs" onClick={() => setShowFilters((v) => !v)}>
                {t('users.filter_toggle')}
                {hasActive && <span className="ml-1.5 h-1.5 w-1.5 rounded-full bg-primary inline-block" />}
              </Button>
            </div>
            <Input
              placeholder={t('users.search_placeholder')}
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              className="h-9 mt-2"
            />
            <div className="flex items-center gap-1.5 mt-2">
              <Select value={sort} onValueChange={(v) => { setSort(v as typeof sort); setPage(1) }}>
                <SelectTrigger className="h-7 flex-1 text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="created_at">{t('users.sort_joined')}</SelectItem>
                  <SelectItem value="updated_at">{t('users.sort_last_seen')}</SelectItem>
                  <SelectItem value="name">{t('users.sort_name')}</SelectItem>
                  <SelectItem value="role">{t('users.sort_role')}</SelectItem>
                  <SelectItem value="dl_count">{t('users.sort_dl_count')}</SelectItem>
                  <SelectItem value="dl_last_at">{t('users.sort_dl_last')}</SelectItem>
                </SelectContent>
              </Select>
              <Button
                variant="outline"
                size="icon"
                className="h-7 w-7 shrink-0"
                onClick={() => { setSortDir((d) => d === 'desc' ? 'asc' : 'desc'); setPage(1) }}
              >
                {sortDir === 'desc'
                  ? <ArrowDownAZ className="h-3.5 w-3.5" />
                  : <ArrowUpAZ className="h-3.5 w-3.5" />}
              </Button>
            </div>
          </CardHeader>

          {showFilters && (
            <div className="px-4 pb-3 space-y-2 border-t pt-3">
              <div className="grid grid-cols-2 gap-2">
                <Select value={filters.role} onValueChange={(v) => { setFilters((f) => ({ ...f, role: v })); setPage(1) }}>
                  <SelectTrigger className="h-9"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">{t('users.filter_all_roles')}</SelectItem>
                    {ROLES.map((r) => <SelectItem key={r} value={r}>{r}</SelectItem>)}
                  </SelectContent>
                </Select>
                <Select value={filters.status} onValueChange={(v) => { setFilters((f) => ({ ...f, status: v as StatusFilter })); setPage(1) }}>
                  <SelectTrigger className="h-9"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="all">{t('users.filter_all')}</SelectItem>
                    <SelectItem value="active">{t('users.filter_active')}</SelectItem>
                    <SelectItem value="restricted">{t('users.filter_restricted')}</SelectItem>
                    <SelectItem value="banned">{t('users.filter_banned')}</SelectItem>
                  </SelectContent>
                </Select>
              </div>
              {hasActive && (
                <Button size="sm" variant="outline" className="h-8 w-full text-xs" onClick={resetFilters}>
                  {t('users.filter_clear')}
                </Button>
              )}
            </div>
          )}

          <CardContent className={cn('p-0 transition-opacity duration-150', fetching && !initialLoading && 'opacity-60')}>
            {initialLoading ? (
              <div className="divide-y divide-border px-3 py-1">
                {Array.from({ length: 5 }).map((_, i) => (
                  <div key={i} className="flex items-center gap-3 py-2.5">
                    <Skeleton className="size-8 rounded-md shrink-0" />
                    <div className="flex-1 space-y-1.5">
                      <Skeleton className="h-3.5 w-28" />
                      <Skeleton className="h-3 w-20" />
                    </div>
                    <Skeleton className="h-5 w-14" />
                  </div>
                ))}
              </div>
            ) : items.length === 0 ? (
              <div className="flex justify-center py-10 text-sm text-muted-foreground">{t('users.no_users')}</div>
            ) : (
              <div className="divide-y divide-border px-3 py-1">
                {items.map((user) => (
                  <Item
                    key={user.id}
                    size="sm"
                    className="py-2.5 rounded-none border-0 cursor-pointer"
                    onClick={() => setViewed(user)}
                  >
                    <Avatar className={cn('size-8 ring-1', RING_BY_ROLE[user.role] ?? RING_BY_ROLE.user)}>
                      <AvatarImage src={userPhotoUrl(user.id)} />
                      <AvatarFallback className="text-xs font-medium text-primary">
                        {userInitials(user)}
                      </AvatarFallback>
                    </Avatar>
                    <ItemContent>
                      <ItemTitle>{user.first_name ?? user.username ?? String(user.id)}</ItemTitle>
                      <ItemDescription>
                        {user.username ? `@${user.username}` : `#${user.id}`}
                      </ItemDescription>
                    </ItemContent>
                    <ItemActions>
                      <RoleBadge role={user.role} />
                      <div onClick={(e) => e.stopPropagation()}>
                        {user.role === 'banned' ? (
                          <Tooltip>
                            <TooltipTrigger asChild>
                              <Button variant="ghost" size="icon" className="h-8 w-8 text-green-600 hover:text-green-700" onClick={() => void quickUnban(user)}>
                                <ShieldCheck className="h-4 w-4" />
                              </Button>
                            </TooltipTrigger>
                            <TooltipContent>{t('users.quick_unban')}</TooltipContent>
                          </Tooltip>
                        ) : user.role !== 'owner' ? (
                          <Tooltip>
                            <TooltipTrigger asChild>
                              <Button variant="ghost" size="icon" className="h-8 w-8 text-muted-foreground hover:text-destructive" onClick={() => void quickBan(user)}>
                                <Ban className="h-4 w-4" />
                              </Button>
                            </TooltipTrigger>
                            <TooltipContent>{t('users.quick_ban')}</TooltipContent>
                          </Tooltip>
                        ) : null}
                      </div>
                    </ItemActions>
                  </Item>
                ))}
              </div>
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

        <UserDrawer user={viewed} onClose={() => setViewed(null)} onUpdated={handleUpdated} />
      </div>
    </TooltipProvider>
  )
}
