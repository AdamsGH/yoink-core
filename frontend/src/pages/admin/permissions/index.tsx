import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { apiClient } from '@core/lib/api-client'
import { formatDate } from '@core/lib/utils'
import type { Feature, Permission, User } from '@core/types/api'
import { Badge } from '@core/components/ui/badge'
import { Tooltip, TooltipContent, TooltipTrigger } from '@core/components/ui/tooltip'
import { Button } from '@core/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@core/components/ui/card'
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@core/components/ui/dialog'
import { Input } from '@core/components/ui/input'
import { Label } from '@core/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@core/components/ui/select'
import { Skeleton } from '@core/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@core/components/ui/table'
import { toast } from '@core/components/ui/toast'

interface PermissionWithUser extends Permission {
  username?: string | null
  first_name?: string | null
}

export default function AdminPermissionsPage() {
  const { t } = useTranslation()

  const [features, setFeatures] = useState<Feature[]>([])
  const [permissions, setPermissions] = useState<PermissionWithUser[]>([])
  const [usersMap, setUsersMap] = useState<Map<number, User>>(new Map())
  const [loading, setLoading] = useState(true)

  const [filterPlugin, setFilterPlugin] = useState<string>('all')
  const [filterFeature, setFilterFeature] = useState<string>('all')

  const [grantOpen, setGrantOpen] = useState(false)
  const [grantPlugin, setGrantPlugin] = useState('')
  const [grantFeature, setGrantFeature] = useState('')
  const [grantUserId, setGrantUserId] = useState('')
  const [grantUserSearch, setGrantUserSearch] = useState('')
  const [grantUserResults, setGrantUserResults] = useState<User[]>([])
  const [grantSaving, setGrantSaving] = useState(false)

  const loadData = async () => {
    setLoading(true)
    try {
      const [featRes, permRes] = await Promise.all([
        apiClient.get<Feature[]>('/features'),
        apiClient.get<Permission[]>('/permissions/all'),
      ])
      setFeatures(featRes.data)

      const perms = permRes.data
      const userIds = [...new Set(perms.flatMap((p) => [p.user_id, p.granted_by]))]
      const map = new Map<number, User>()
      await Promise.all(
        userIds.map((id) =>
          apiClient.get<User>(`/users/${id}`).then((r) => map.set(id, r.data)).catch(() => null)
        )
      )
      setUsersMap(map)
      setPermissions(perms.map((p) => ({
        ...p,
        username: map.get(p.user_id)?.username,
        first_name: map.get(p.user_id)?.first_name,
      })))
    } catch {
      toast.error(t('permissions.load_error'))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => { void loadData() }, [])

  const uniquePlugins = [...new Set(features.map((f) => f.plugin))]
  const featuresForPlugin = features.filter(
    (f) => filterPlugin === 'all' || f.plugin === filterPlugin
  )

  const displayed = permissions.filter((p) => {
    if (filterPlugin !== 'all' && p.plugin !== filterPlugin) return false
    if (filterFeature !== 'all' && p.feature !== filterFeature) return false
    return true
  })

  const handleRevoke = async (p: Permission) => {
    try {
      await apiClient.delete(`/users/${p.user_id}/permissions/${p.plugin}/${p.feature}`)
      toast.success(t('permissions.revoked'))
      void loadData()
    } catch {
      toast.error(t('permissions.revoke_error'))
    }
  }

  const searchUsers = async (q: string) => {
    if (q.length < 2) { setGrantUserResults([]); return }
    try {
      const res = await apiClient.get<{ items: User[] }>('/users', { params: { search: q, limit: 10 } })
      setGrantUserResults(res.data.items ?? [])
    } catch { setGrantUserResults([]) }
  }

  useEffect(() => {
    const timer = setTimeout(() => void searchUsers(grantUserSearch), 300)
    return () => clearTimeout(timer)
  }, [grantUserSearch])

  const handleGrant = async () => {
    if (!grantUserId || !grantPlugin || !grantFeature) return
    setGrantSaving(true)
    try {
      await apiClient.post(`/users/${grantUserId}/permissions`, {
        plugin: grantPlugin,
        feature: grantFeature,
      })
      toast.success(t('permissions.granted'))
      setGrantOpen(false)
      setGrantUserId('')
      setGrantUserSearch('')
      setGrantPlugin('')
      setGrantFeature('')
      void loadData()
    } catch {
      toast.error(t('permissions.grant_error'))
    } finally {
      setGrantSaving(false)
    }
  }

  const displayUser = (p: PermissionWithUser) =>
    p.username ? `@${p.username}` : (p.first_name ?? String(p.user_id))

  const displayGrantedBy = (id: number) => {
    const u = usersMap.get(id)
    return u?.username ? `@${u.username}` : (u?.first_name ?? String(id))
  }

  return (
    <div className="space-y-4 p-4">
      <div className="flex items-center justify-end">
        <Button size="sm" className="h-7 px-2.5 text-xs" onClick={() => setGrantOpen(true)}>
          {t('permissions.grant_button')}
        </Button>
      </div>

      <Card>
        <CardHeader className="px-4 py-3">
          <CardTitle className="text-sm font-medium text-muted-foreground">
            {t('permissions.available_features')}
          </CardTitle>
        </CardHeader>
        <CardContent>
          <div className="flex flex-wrap gap-2">
            {features.map((f) => (
              <div key={`${f.plugin}.${f.feature}`} className="flex flex-col rounded-md border px-3 py-1.5 text-xs">
                <span className="font-medium">{f.label}</span>
                <span className="text-muted-foreground">{f.plugin}/{f.feature}</span>
                {f.description && <span className="mt-0.5 text-muted-foreground/70">{f.description}</span>}
              </div>
            ))}
          </div>
        </CardContent>
      </Card>

      <div className="flex gap-2">
        <Select value={filterPlugin} onValueChange={(v) => { setFilterPlugin(v); setFilterFeature('all') }}>
          <SelectTrigger className="w-36">
            <SelectValue placeholder={t('permissions.filter_plugin')} />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">{t('permissions.all_plugins')}</SelectItem>
            {uniquePlugins.map((p) => <SelectItem key={p} value={p}>{p}</SelectItem>)}
          </SelectContent>
        </Select>
        <Select value={filterFeature} onValueChange={setFilterFeature} disabled={filterPlugin === 'all'}>
          <SelectTrigger className="w-40">
            <SelectValue placeholder={t('permissions.filter_feature')} />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">{t('permissions.all_features')}</SelectItem>
            {featuresForPlugin.map((f) => (
              <SelectItem key={f.feature} value={f.feature}>{f.label}</SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <Card>
        <CardContent className="p-0">
          {loading ? (
            <div className="space-y-2 p-4">
              {Array.from({ length: 5 }).map((_, i) => <Skeleton key={i} className="h-8 w-full" />)}
            </div>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>{t('permissions.col_user')}</TableHead>
                  <TableHead>{t('permissions.col_plugin')}</TableHead>
                  <TableHead>{t('permissions.col_feature')}</TableHead>
                  <TableHead>{t('permissions.col_granted_by')}</TableHead>
                  <TableHead>{t('permissions.col_granted_at')}</TableHead>
                  <TableHead>{t('permissions.col_expires')}</TableHead>
                  <TableHead />
                </TableRow>
              </TableHeader>
              <TableBody>
                {displayed.length === 0 ? (
                  <TableRow>
                    <TableCell colSpan={7} className="text-center text-muted-foreground py-6">
                      {t('permissions.empty')}
                    </TableCell>
                  </TableRow>
                ) : displayed.map((p) => (
                  <TableRow key={p.id}>
                    <TableCell className="font-medium">{displayUser(p)}</TableCell>
                    <TableCell><Badge variant="outline">{p.plugin}</Badge></TableCell>
                    <TableCell>
                      <div className="flex items-center gap-1.5">
                        {features.find((f) => f.plugin === p.plugin && f.feature === p.feature)?.label ?? p.feature}
                        {p.grant_source === 'tag' && (
                          <Tooltip>
                            <TooltipTrigger asChild>
                              <Badge variant="secondary" className="cursor-default text-xs px-1.5 py-0">
                                {t('permissions.source_tag')}
                              </Badge>
                            </TooltipTrigger>
                            <TooltipContent side="top">
                              {t('permissions.source_tag_hint')}
                            </TooltipContent>
                          </Tooltip>
                        )}
                      </div>
                    </TableCell>
                    <TableCell className="text-muted-foreground text-sm">{displayGrantedBy(p.granted_by)}</TableCell>
                    <TableCell className="text-sm">{formatDate(p.granted_at)}</TableCell>
                    <TableCell className="text-sm text-muted-foreground">
                      {p.expires_at ? formatDate(p.expires_at) : t('permissions.never_expires')}
                    </TableCell>
                    <TableCell>
                      {p.grant_source === 'tag' ? (
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <span>
                              <Button
                                variant="ghost"
                                size="sm"
                                className="text-muted-foreground cursor-not-allowed opacity-40"
                                disabled
                              >
                                {t('permissions.revoke')}
                              </Button>
                            </span>
                          </TooltipTrigger>
                          <TooltipContent side="left">
                            {t('permissions.source_tag_no_revoke')}
                          </TooltipContent>
                        </Tooltip>
                      ) : (
                        <Button
                          variant="ghost"
                          size="sm"
                          className="text-destructive hover:text-destructive"
                          onClick={() => void handleRevoke(p)}
                        >
                          {t('permissions.revoke')}
                        </Button>
                      )}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>

      <Dialog open={grantOpen} onOpenChange={setGrantOpen}>
        <DialogContent className="max-w-sm">
          <DialogHeader>
            <DialogTitle>{t('permissions.grant_title')}</DialogTitle>
          </DialogHeader>
          <div className="space-y-3">
            <div className="space-y-1">
              <Label>{t('permissions.grant_user')}</Label>
              <Input
                placeholder={t('permissions.grant_user_placeholder')}
                value={grantUserSearch}
                onChange={(e) => setGrantUserSearch(e.target.value)}
              />
              {grantUserResults.length > 0 && (
                <div className="rounded-md border bg-popover shadow-md">
                  {grantUserResults.map((u) => (
                    <button
                      key={u.id}
                      type="button"
                      className="flex w-full items-center gap-2 px-3 py-1.5 text-sm hover:bg-muted"
                      onClick={() => {
                        setGrantUserId(String(u.id))
                        setGrantUserSearch(u.username ? `@${u.username}` : (u.first_name ?? String(u.id)))
                        setGrantUserResults([])
                      }}
                    >
                      <span>{u.username ? `@${u.username}` : (u.first_name ?? String(u.id))}</span>
                      <span className="text-muted-foreground text-xs">#{u.id}</span>
                    </button>
                  ))}
                </div>
              )}
            </div>
            <div className="space-y-1">
              <Label>{t('permissions.grant_plugin')}</Label>
              <Select value={grantPlugin} onValueChange={(v) => { setGrantPlugin(v); setGrantFeature('') }}>
                <SelectTrigger>
                  <SelectValue placeholder={t('permissions.select_plugin')} />
                </SelectTrigger>
                <SelectContent>
                  {uniquePlugins.map((p) => <SelectItem key={p} value={p}>{p}</SelectItem>)}
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-1">
              <Label>{t('permissions.grant_feature')}</Label>
              <Select value={grantFeature} onValueChange={setGrantFeature} disabled={!grantPlugin}>
                <SelectTrigger>
                  <SelectValue placeholder={t('permissions.select_feature')} />
                </SelectTrigger>
                <SelectContent>
                  {features
                    .filter((f) => f.plugin === grantPlugin)
                    .map((f) => (
                      <SelectItem key={f.feature} value={f.feature}>{f.label}</SelectItem>
                    ))}
                </SelectContent>
              </Select>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setGrantOpen(false)}>{t('common.cancel')}</Button>
            <Button
              onClick={() => void handleGrant()}
              disabled={!grantUserId || !grantPlugin || !grantFeature || grantSaving}
            >
              {grantSaving ? t('common.saving') : t('permissions.grant_button')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
