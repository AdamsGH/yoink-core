import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { format } from 'date-fns'
import { CalendarIcon, X } from 'lucide-react'

import { apiClient } from '@core/lib/api-client'
import { cn, formatDate } from '@core/lib/utils'
import { GRADIENT_BY_ROLE, RING_BY_ROLE, userInitials, userPhotoUrl } from '@core/lib/user-utils'
import type { EffectiveFeatureAccess, User, UserRole, UserUpdateRequest } from '@core/types/api'
import { Avatar, AvatarFallback, AvatarImage } from '@core/components/ui/avatar'
import { Badge } from '@core/components/ui/badge'
import { RoleBadge } from '@core/components/app/StatusBadge'
import { Button } from '@core/components/ui/button'
import { Calendar } from '@core/components/ui/calendar'
import { Drawer, DrawerContent } from '@core/components/ui/drawer'
import { Label } from '@core/components/ui/label'
import { Popover, PopoverContent, PopoverTrigger } from '@core/components/ui/popover'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@core/components/ui/select'
import { Skeleton } from '@core/components/ui/skeleton'
import { Switch } from '@core/components/ui/switch'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@core/components/ui/tabs'
import { toast } from '@core/components/ui/toast'

function BanDatePicker({
  value,
  onChange,
}: {
  value: string
  onChange: (iso: string) => void
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const [hour, setHour] = useState('23')
  const [minute, setMinute] = useState('59')

  const selected = value ? new Date(value) : undefined

  const applyTime = (day: Date, h: string, m: string) => {
    const d = new Date(day)
    d.setHours(parseInt(h, 10), parseInt(m, 10), 0, 0)
    onChange(d.toISOString())
  }

  const handleDaySelect = (day: Date | undefined) => {
    if (!day) { onChange(''); setOpen(false); return }
    applyTime(day, hour, minute)
    setOpen(false)
  }

  const handleHourChange = (h: string) => {
    setHour(h)
    if (selected) applyTime(selected, h, minute)
  }

  const handleMinuteChange = (m: string) => {
    setMinute(m)
    if (selected) applyTime(selected, hour, m)
  }

  const label = selected
    ? format(selected, 'MMM d, yyyy HH:mm')
    : t('users.ban_until_placeholder', { defaultValue: 'Pick date…' })

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          className={cn('w-full justify-start gap-2 font-normal', !selected && 'text-muted-foreground')}
        >
          <CalendarIcon className="h-4 w-4 shrink-0" />
          <span className="flex-1 text-left truncate">{label}</span>
          {selected && (
            <X
              className="h-3.5 w-3.5 shrink-0 opacity-50 hover:opacity-100"
              onClick={(e) => { e.stopPropagation(); onChange('') }}
            />
          )}
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-auto p-0" align="start">
        <Calendar
          mode="single"
          selected={selected}
          onSelect={handleDaySelect}
          disabled={(d) => d < new Date()}
          initialFocus
        />
        <div className="border-t p-3 flex items-center gap-2">
          <Label className="text-xs text-muted-foreground shrink-0">
            {t('users.ban_until_time', { defaultValue: 'Time' })}
          </Label>
          <Select value={hour} onValueChange={handleHourChange}>
            <SelectTrigger className="h-8 w-20">
              <SelectValue />
            </SelectTrigger>
            <SelectContent className="max-h-48">
              {Array.from({ length: 24 }, (_, i) => String(i).padStart(2, '0')).map(h => (
                <SelectItem key={h} value={h}>{h}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          <span className="text-muted-foreground">:</span>
          <Select value={minute} onValueChange={handleMinuteChange}>
            <SelectTrigger className="h-8 w-20">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {['00', '15', '30', '45'].map(m => (
                <SelectItem key={m} value={m}>{m}</SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </PopoverContent>
    </Popover>
  )
}

const SUPPORTED_LANGS = ['en', 'ru']
const ROLES: UserRole[] = ['owner', 'admin', 'moderator', 'user', 'restricted', 'banned']

interface UserStats {
  total: number
  this_week: number
  today: number
  top_domains: { domain: string; count: number }[]
  member_since: string
  dl_last_at: string | null
  music_total: number
  music_last_at: string | null
  ai_total: number
  ai_last_at: string | null
}

function StatCell({ label, value }: { label: string; value: string | number }) {
  return (
    <div className="rounded-lg border bg-card p-3 text-center">
      <div className="text-2xl font-bold tabular-nums">{value}</div>
      <div className="text-[11px] text-muted-foreground mt-1 uppercase tracking-wide">{label}</div>
    </div>
  )
}

function AccessBadge({ f }: { f: EffectiveFeatureAccess }) {
  if (!f.effective) {
    return <span className="text-xs text-muted-foreground">-</span>
  }
  if (f.access_via_role && !f.access_via_grant) {
    return <Badge variant="outline" className="text-[10px] px-1.5 py-0 border-blue-500/50 text-blue-600">role</Badge>
  }
  if (f.access_via_grant && f.grant_source === 'tag') {
    return <Badge variant="secondary" className="text-[10px] px-1.5 py-0">tag</Badge>
  }
  if (!f.access_via_role && f.access_via_grant) {
    return <Badge variant="outline" className="text-[10px] px-1.5 py-0 border-green-500/50 text-green-600">grant</Badge>
  }
  return <Badge variant="outline" className="text-[10px] px-1.5 py-0 border-green-500/50 text-green-600">role+grant</Badge>
}

function roleMediaColor(role: UserRole) {
  if (role === 'banned') return 'bg-destructive/10 text-destructive'
  if (role === 'owner') return 'bg-amber-500/10 text-amber-600'
  if (role === 'admin') return 'bg-blue-500/10 text-blue-600'
  if (role === 'moderator') return 'bg-purple-500/10 text-purple-600'
  if (role === 'restricted') return 'bg-orange-500/10 text-orange-600'
  return 'bg-muted text-muted-foreground'
}

export function UserDrawer({
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

  const [features, setFeatures] = useState<EffectiveFeatureAccess[]>([])
  const [permsLoading, setPermsLoading] = useState(false)
  const [togglingId, setTogglingId] = useState<string | null>(null)

  const [editRole, setEditRole] = useState<UserRole>('user')
  const [editLang, setEditLang] = useState('en')
  const [banUntil, setBanUntil] = useState('')
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    if (!user) { setStats(null); setFeatures([]); return }

    setEditRole(user.role)
    setEditLang(user.language ?? 'en')
    setBanUntil(user.ban_until ?? '')

    setStatsLoading(true)
    apiClient
      .get<UserStats>(`/users/${user.id}/stats`)
      .then((r) => setStats(r.data))
      .catch(() => {})
      .finally(() => setStatsLoading(false))

    setPermsLoading(true)
    apiClient
      .get<EffectiveFeatureAccess[]>(`/users/${user.id}/feature-access`)
      .then((r) => setFeatures(r.data))
      .catch(() => {})
      .finally(() => setPermsLoading(false))
  }, [user?.id])

  const toggleFeature = async (f: EffectiveFeatureAccess, grant: boolean) => {
    if (!user) return
    const key = `${f.plugin}.${f.feature}`
    setTogglingId(key)
    try {
      if (grant) {
        await apiClient.post(`/users/${user.id}/permissions`, { plugin: f.plugin, feature: f.feature })
        toast.success(t('permissions.granted'))
      } else {
        await apiClient.delete(`/users/${user.id}/permissions/${f.plugin}/${f.feature}`)
        toast.success(t('permissions.revoked'))
      }
      const r = await apiClient.get<EffectiveFeatureAccess[]>(`/users/${user.id}/feature-access`)
      setFeatures(r.data)
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
      if (editLang !== user.language) body.language = editLang
      body.ban_until = banUntil || null
      const res = await apiClient.patch<User>(`/users/${user.id}`, body)
      toast.success(t('users.update_success'))
      onUpdated(res.data)
    } catch {
      toast.error(t('users.update_error'))
    } finally {
      setSaving(false)
    }
  }

  const groupedFeatures = features.reduce<Record<string, EffectiveFeatureAccess[]>>((acc, f) => {
    (acc[f.plugin] ??= []).push(f)
    return acc
  }, {})

  return (
    <Drawer open={!!user} onOpenChange={(o) => !o && onClose()}>
      <DrawerContent className="max-h-[85vh] border-0">
        {user && (
          <>
            <div className={cn('bg-gradient-to-b -mt-7 pt-7 px-4 pb-5 border-b border-border/50 rounded-t-[10px]', GRADIENT_BY_ROLE[user.role] ?? GRADIENT_BY_ROLE.user)}>
              <div className="flex items-center gap-4">
                <Avatar className={cn('size-16 ring-2 shadow-lg', RING_BY_ROLE[user.role] ?? RING_BY_ROLE.user)}>
                  <AvatarImage src={userPhotoUrl(user.id)} />
                  <AvatarFallback className={cn('text-xl font-bold', roleMediaColor(user.role))}>
                    {userInitials(user)}
                  </AvatarFallback>
                </Avatar>
                <div className="min-w-0 flex-1 space-y-1">
                  <div className="flex items-center gap-2">
                    <h3 className="text-lg font-semibold truncate leading-tight">{user.first_name ?? String(user.id)}</h3>
                    <RoleBadge role={user.role} />
                  </div>
                  <div className="flex items-center gap-2 text-sm text-muted-foreground">
                    {user.username && <span>@{user.username}</span>}
                    <span className="font-mono text-xs">#{user.id}</span>
                  </div>
                </div>
              </div>
            </div>

            <Tabs defaultValue="stats" className="flex flex-col flex-1 min-h-0">
              <TabsList className="mx-4 mt-3 shrink-0">
                <TabsTrigger value="stats" className="flex-1">{t('users.tab_stats')}</TabsTrigger>
                <TabsTrigger value="access" className="flex-1">{t('users.tab_access')}</TabsTrigger>
                <TabsTrigger value="edit" className="flex-1">{t('users.tab_edit')}</TabsTrigger>
              </TabsList>

              <div className="flex-1 overflow-y-auto pb-6">
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
                      {stats.music_total > 0 && (
                        <div className="rounded-md border px-3 py-2 space-y-1">
                          <p className="text-xs font-medium">{t('users.music_section', { defaultValue: 'Music' })}</p>
                          <div className="flex justify-between text-xs text-muted-foreground">
                            <span>{t('users.music_total', { defaultValue: 'Resolved' })}</span>
                            <span className="font-medium text-foreground">{stats.music_total.toLocaleString()}</span>
                          </div>
                          {stats.music_last_at && (
                            <div className="flex justify-between text-xs text-muted-foreground">
                              <span>{t('users.music_last', { defaultValue: 'Last used' })}</span>
                              <span>{formatDate(stats.music_last_at)}</span>
                            </div>
                          )}
                        </div>
                      )}
                      {stats.ai_total > 0 && (
                        <div className="rounded-md border px-3 py-2 space-y-1">
                          <p className="text-xs font-medium">{t('users.ai_section', { defaultValue: 'AI / Insight' })}</p>
                          <div className="flex justify-between text-xs text-muted-foreground">
                            <span>{t('users.ai_total', { defaultValue: 'Queries' })}</span>
                            <span className="font-medium text-foreground">{stats.ai_total.toLocaleString()}</span>
                          </div>
                          {stats.ai_last_at && (
                            <div className="flex justify-between text-xs text-muted-foreground">
                              <span>{t('users.ai_last', { defaultValue: 'Last used' })}</span>
                              <span>{formatDate(stats.ai_last_at)}</span>
                            </div>
                          )}
                        </div>
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
                        {stats.dl_last_at && (
                          <div className="flex justify-between">
                            <span>{t('users.dl_last_at', { defaultValue: 'Last download' })}</span>
                            <span>{formatDate(stats.dl_last_at)}</span>
                          </div>
                        )}
                      </div>
                    </>
                  ) : (
                    <p className="text-sm text-muted-foreground text-center py-4">{t('common.load_error')}</p>
                  )}
                </TabsContent>

                <TabsContent value="access" className="px-4 py-3 space-y-4 mt-0">
                  {permsLoading ? (
                    <div className="space-y-3">
                      {[0, 1].map((i) => <Skeleton key={i} className="h-16 w-full rounded-md" />)}
                    </div>
                  ) : features.length === 0 ? (
                    <p className="text-sm text-muted-foreground text-center py-4">{t('permissions.no_features')}</p>
                  ) : (
                    Object.entries(groupedFeatures).map(([plugin, feats]) => (
                      <div key={plugin} className="space-y-1.5">
                        <p className="text-xs font-semibold text-muted-foreground uppercase tracking-wide">{plugin}</p>
                        <div className="rounded-md border divide-y">
                          {feats.map((f) => {
                            const key = `${f.plugin}.${f.feature}`
                            return (
                              <div key={key} className="px-3 py-2.5 space-y-1">
                                <div className="flex items-center justify-between gap-2">
                                  <div className="min-w-0 flex-1">
                                    <div className="flex items-center gap-2">
                                      <p className="text-sm font-medium">{f.label}</p>
                                      <AccessBadge f={f} />
                                    </div>
                                    {f.description && (
                                      <p className="text-xs text-muted-foreground">{f.description}</p>
                                    )}
                                    {f.default_min_role && (
                                      <p className="text-xs text-muted-foreground">
                                        {t('permissions.default_role')}: <span className="font-mono">{f.default_min_role}+</span>
                                      </p>
                                    )}
                                  </div>
                                  <Switch
                                    checked={f.effective}
                                    disabled={togglingId === key || f.access_via_role || f.grant_source === 'tag'}
                                    title={
                                      f.access_via_role
                                        ? t('permissions.granted_by_role')
                                        : f.grant_source === 'tag'
                                          ? t('permissions.source_tag_no_revoke')
                                          : undefined
                                    }
                                    onCheckedChange={(v) => void toggleFeature(f, v)}
                                  />
                                </div>
                                {f.access_via_grant && f.grant_expires_at && (
                                  <p className="text-xs text-muted-foreground pl-0">
                                    {t('permissions.expires')}: {formatDate(f.grant_expires_at)}
                                  </p>
                                )}
                              </div>
                            )
                          })}
                        </div>
                      </div>
                    ))
                  )}
                </TabsContent>

                <TabsContent value="edit" className="px-4 py-3 space-y-4 mt-0">
                  {user.role === 'owner' && (
                    <div className="rounded-md border border-amber-500/30 bg-amber-500/5 px-3 py-2 text-xs text-amber-600 dark:text-amber-400">
                      {t('users.owner_protected', { defaultValue: 'Owner role is assigned via config and cannot be changed.' })}
                    </div>
                  )}
                  <div className="space-y-1.5">
                    <Label>{t('users.col_role')}</Label>
                    <Select value={editRole} onValueChange={(v) => setEditRole(v as UserRole)} disabled={user.role === 'owner'}>
                      <SelectTrigger><SelectValue /></SelectTrigger>
                      <SelectContent>
                        {(user.role === 'owner' ? ROLES : ROLES.filter((r) => r !== 'owner')).map((r) => (
                          <SelectItem key={r} value={r}>{r}</SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                  <div className="space-y-1.5">
                    <Label>{t('users.col_language')}</Label>
                    <Select value={editLang} onValueChange={setEditLang}>
                      <SelectTrigger><SelectValue /></SelectTrigger>
                      <SelectContent>
                        {SUPPORTED_LANGS.map((l) => (
                          <SelectItem key={l} value={l}>{l.toUpperCase()}</SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                  {user.role !== 'owner' && (
                    <div className="space-y-1.5">
                      <Label>{t('users.ban_until_label')}</Label>
                      <BanDatePicker value={banUntil} onChange={setBanUntil} />
                      <p className="text-xs text-muted-foreground">{t('users.ban_until_hint')}</p>
                    </div>
                  )}
                  <Button className="w-full" onClick={saveUser} disabled={saving || user.role === 'owner'}>
                    {saving ? t('common.saving') : t('common.save')}
                  </Button>
                </TabsContent>
              </div>
            </Tabs>
          </>
        )}
      </DrawerContent>
    </Drawer>
  )
}

