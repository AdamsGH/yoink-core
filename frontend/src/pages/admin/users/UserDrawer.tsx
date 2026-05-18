import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { usersApi } from '@core/lib/api'
import { cn } from '@core/lib/utils'
import { GRADIENT_BY_ROLE, RING_BY_ROLE, userInitials, userPhotoUrl } from '@core/lib/user-utils'
import type { EffectiveFeatureAccess, User, UserRole, UserUpdateRequest } from '@core/types/api'
import { Avatar, AvatarFallback, AvatarImage, Button, Drawer, DrawerContent, Label, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Tabs, TabsContent, TabsList, TabsTrigger } from '@ui'
import { RoleBadge } from '@app'
import { toast } from '@core/components/ui/toast'

import { BanDatePicker } from './BanDatePicker'
import { StatsTab } from './UserDrawerStatsTab'
import type { UserStats } from './UserDrawerStatsTab'
import { PermissionsTab, roleMediaColor } from './UserDrawerPermissionsTab'

const SUPPORTED_LANGS = ['en', 'ru']
const ROLES: UserRole[] = ['owner', 'admin', 'moderator', 'user', 'restricted', 'banned']

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
    usersApi
      .getStats(user.id)
      .then((r) => setStats(r.data))
      .catch(() => {})
      .finally(() => setStatsLoading(false))

    setPermsLoading(true)
    usersApi
      .getFeatureAccess(user.id)
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
        await usersApi.grantPermission(user.id, f.plugin, f.feature)
        toast.success(t('permissions.granted'))
      } else {
        await usersApi.revokePermission(user.id, f.plugin, f.feature)
        toast.success(t('permissions.revoked'))
      }
      const r = await usersApi.getFeatureAccess(user.id)
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
      const res = await usersApi.update(user.id, body)
      toast.success(t('users.update_success'))
      onUpdated(res.data)
    } catch {
      toast.error(t('users.update_error'))
    } finally {
      setSaving(false)
    }
  }

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
                <TabsContent value="stats" className="mt-0">
                  <StatsTab user={user} stats={stats} statsLoading={statsLoading} />
                </TabsContent>

                <TabsContent value="access" className="mt-0">
                  <PermissionsTab
                    features={features}
                    permsLoading={permsLoading}
                    togglingId={togglingId}
                    onToggle={toggleFeature}
                  />
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
