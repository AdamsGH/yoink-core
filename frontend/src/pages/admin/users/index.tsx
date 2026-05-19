import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { ArrowDownAZ, ArrowUpAZ, Ban, ShieldCheck, Users } from 'lucide-react'

import { cn } from '@core/lib/utils'
import { RING_BY_ROLE, userInitials, userPhotoUrl } from '@core/lib/user-utils'
import type { UserRole } from '@core/types/api'
import { Avatar, AvatarFallback, AvatarImage, Button, Card, CardContent, CardHeader, CardTitle, Input, Item, ItemActions, ItemContent, ItemDescription, ItemTitle, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Skeleton, Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@ui'
import { RoleBadge } from '@app'
import { UserDrawer } from './UserDrawer'
import { useAdminUsers, type StatusFilter, type UserSortField, type UserPeriod } from './useAdminUsers'

const ROLES: UserRole[] = ['owner', 'admin', 'moderator', 'user', 'restricted', 'banned']

export default function AdminUsersPage() {
  const { t } = useTranslation()
  const {
    items, total, totalPages,
    initialLoading, fetching,
    page, setPage,
    search, setSearch,
    filters, setFilters, hasActive, resetFilters,
    sort, setSort, sortDir, toggleSortDir,
    period, setPeriod,
    viewed, setViewed, handleUpdated,
    quickBan, quickUnban,
  } = useAdminUsers()

  const [showFilters, setShowFilters] = useState(false)

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
              <Select value={sort} onValueChange={(v) => setSort(v as UserSortField)}>
                <SelectTrigger className="h-7 flex-1 text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="dl_count">{t('users.sort_dl_count')}</SelectItem>
                  <SelectItem value="dl_last_at">{t('users.sort_dl_last')}</SelectItem>
                  <SelectItem value="created_at">{t('users.sort_joined')}</SelectItem>
                  <SelectItem value="updated_at">{t('users.sort_last_seen')}</SelectItem>
                  <SelectItem value="name">{t('users.sort_name')}</SelectItem>
                  <SelectItem value="role">{t('users.sort_role')}</SelectItem>
                </SelectContent>
              </Select>
              {(sort === 'dl_count' || sort === 'dl_last_at') && (
                <Select value={period} onValueChange={(v) => setPeriod(v as UserPeriod)}>
                  <SelectTrigger className="h-7 w-20 text-xs shrink-0">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="7">{t('users.period_7', { defaultValue: '7d' })}</SelectItem>
                    <SelectItem value="30">{t('users.period_30', { defaultValue: '30d' })}</SelectItem>
                    <SelectItem value="90">{t('users.period_90', { defaultValue: '90d' })}</SelectItem>
                    <SelectItem value="all">{t('users.period_all', { defaultValue: 'all time' })}</SelectItem>
                  </SelectContent>
                </Select>
              )}
              <Button
                variant="outline"
                size="icon"
                className="h-7 w-7 shrink-0"
                onClick={toggleSortDir}
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
