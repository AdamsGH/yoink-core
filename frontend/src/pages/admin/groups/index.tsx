import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { MessageSquare, Pencil, Plus, Trash2 } from 'lucide-react'

import { apiClient } from '@core/lib/api-client'
import { cn } from '@core/lib/utils'
import type { Group, GroupCreateRequest, GroupUpdateRequest, ThreadPolicy, UserRole } from '@core/types/api'
import { Badge } from '@core/components/ui/badge'
import { SuccessBadge } from '@core/components/app/StatusBadge'
import { Button } from '@core/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@core/components/ui/card'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@core/components/ui/dialog'
import { Input } from '@core/components/ui/input'
import { Item, ItemActions, ItemContent, ItemDescription, ItemMedia, ItemTitle } from '@core/components/ui/item'
import { Label } from '@core/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@core/components/ui/select'
import { Switch } from '@core/components/ui/switch'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@core/components/ui/tooltip'
import { Skeleton } from '@core/components/ui/skeleton'
import { toast } from '@core/components/ui/toast'

const ROLES: UserRole[] = ['owner', 'admin', 'moderator', 'user', 'restricted', 'banned']

interface EditState {
  group: Group | null
  isNew: boolean
  title: string
  enabled: boolean
  auto_grant_role: UserRole
  allow_pm: boolean
  nsfw_allowed: boolean
  storage_chat_id: string
  storage_thread_id: string
  newId: string
}

function defaultEdit(group?: Group): EditState {
  return {
    group: group ?? null,
    isNew: !group,
    title: group?.title ?? '',
    enabled: group?.enabled ?? false,
    auto_grant_role: group?.auto_grant_role ?? 'user',
    allow_pm: group?.allow_pm ?? true,
    nsfw_allowed: group?.nsfw_allowed ?? false,
    storage_chat_id: group?.storage_chat_id != null ? String(group.storage_chat_id) : '',
    storage_thread_id: group?.storage_thread_id != null ? String(group.storage_thread_id) : '',
    newId: '',
  }
}

function parseThreadId(input: string): number | null {
  const trimmed = input.trim()
  if (/^\d+$/.test(trimmed)) return parseInt(trimmed, 10)
  const m = trimmed.match(/t\.me\/c\/\d+\/(\d+)\/(\d+)/)
  if (m) return parseInt(m[1], 10)
  return null
}

function ManualThreadInput({
  value,
  onChange,
  onEnter,
  showHint = true,
}: {
  value: string
  onChange: (v: string) => void
  onEnter: () => void
  showHint?: boolean
}) {
  const parsed = parseThreadId(value)
  const isLink = value.includes('t.me')
  const isValid = parsed !== null

  return (
    <div className="space-y-1.5">
      <Input
        autoFocus
        className="h-7 text-xs font-mono"
        placeholder="Paste message link or thread ID"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => e.key === 'Enter' && onEnter()}
      />
      {value && (
        <p className={cn('text-xs', isValid ? 'text-green-600 dark:text-green-400' : 'text-muted-foreground')}>
          {isValid
            ? `Thread ID: ${parsed}${isLink ? ' (parsed from link)' : ''}`
            : 'Paste a message link (t.me/c/…/thread/msg) or a plain number'}
        </p>
      )}
      {!value && showHint && (
        <p className="text-xs text-muted-foreground">
          Right-click any message in the topic → Copy Message Link.
          Format: t.me/c/…/<b>thread_id</b>/message_id
        </p>
      )}
    </div>
  )
}

function threadLabel(t: ThreadPolicy): string {
  if (t.name) return t.name
  if (t.thread_id == null) return 'Main chat'
  return `Thread #${t.thread_id}`
}

interface AddState {
  linkOrId: string
  name: string
  enabled: boolean
}

const DEFAULT_ADD: AddState = { linkOrId: '', name: '', enabled: true }

function ThreadRows({ groupId }: { groupId: number }) {
  const { t } = useTranslation()
  const [threads, setThreads] = useState<ThreadPolicy[]>([])
  const [loading, setLoading] = useState(true)
  const [adding, setAdding] = useState(false)
  const [form, setForm] = useState<AddState>(DEFAULT_ADD)

  const load = () => {
    setLoading(true)
    apiClient
      .get<ThreadPolicy[]>(`/groups/${groupId}/threads`)
      .then((res) => setThreads(res.data))
      .catch(() => toast.error(t('common.load_error')))
      .finally(() => setLoading(false))
  }

  useEffect(() => { load() }, [groupId])

  const namedAll = threads.filter((t) => t.name && t.thread_id != null)
  const parsedId = parseThreadId(form.linkOrId)

  const openAdd = () => { setForm(DEFAULT_ADD); setAdding(true) }
  const closeAdd = () => { setAdding(false); setForm(DEFAULT_ADD) }

  const addThread = async () => {
    if (parsedId === null) return
    try {
      await apiClient.post(`/groups/${groupId}/threads`, {
        thread_id: parsedId,
        name: form.name.trim() || null,
        enabled: form.enabled,
      })
      toast.success('Thread policy saved')
      closeAdd()
      load()
    } catch { toast.error(t('common.load_error')) }
  }

  const toggle = async (policy: ThreadPolicy) => {
    try {
      await apiClient.post(`/groups/${groupId}/threads`, { thread_id: policy.thread_id, name: policy.name, enabled: !policy.enabled })
      load()
    } catch { toast.error(t('common.load_error')) }
  }

  const remove = async (policy: ThreadPolicy) => {
    if (!confirm(`Remove policy for "${threadLabel(policy)}"?`)) return
    try {
      await apiClient.delete(`/groups/${groupId}/threads/${policy.id}`)
      load()
    } catch { toast.error(t('common.load_error')) }
  }

  if (loading) return <div className="px-4 py-2 text-xs text-muted-foreground">Loading threads…</div>

  return (
    <div className="border-t bg-muted/30 px-4 py-3 space-y-2">
      <div className="flex items-center justify-between">
        <span className="text-xs font-semibold text-muted-foreground uppercase tracking-wide">Thread Policies</span>
        {!adding && (
          <Button size="sm" variant="ghost" className="h-6 gap-1 text-xs" onClick={openAdd}>
            <Plus className="h-3 w-3" /> Add policy
          </Button>
        )}
      </div>

      {threads.length === 0 && !adding && (
        <p className="text-xs text-muted-foreground">
          No thread policies. Topics are detected automatically from service messages.
        </p>
      )}

      <div className="space-y-1">
        {threads.map((t) => (
          <div key={t.id} className="flex items-center gap-2 rounded-md bg-background px-3 py-1.5 text-xs">
            <div className="min-w-0 flex-1">
              <span className="font-medium">{threadLabel(t)}</span>
              {t.name && t.thread_id != null && (
                <span className="ml-2 font-mono text-muted-foreground">#{t.thread_id}</span>
              )}
            </div>
            {t.enabled
              ? <SuccessBadge className="shrink-0 text-xs">allowed</SuccessBadge>
              : <Badge variant="outline" className="shrink-0 text-xs">denied</Badge>
            }
            <Button size="sm" variant="ghost" className="h-6 shrink-0 text-xs" onClick={() => toggle(t)}>
              {t.enabled ? 'Deny' : 'Allow'}
            </Button>
            <Button size="sm" variant="ghost" className="h-6 shrink-0 text-destructive hover:text-destructive" onClick={() => remove(t)}>
              <Trash2 className="h-3 w-3" />
            </Button>
          </div>
        ))}
      </div>

      {adding && (
        <div className="rounded-md border bg-background p-3 space-y-3">
          <div className="space-y-1.5">
            <p className="text-xs font-medium">{t("groups.col_group")}</p>
            {namedAll.length > 0 && (
              <Select
                value={form.linkOrId}
                onValueChange={(v) => {
                  const existing = threads.find((t) => String(t.thread_id) === v)
                  setForm((f) => ({ ...f, linkOrId: v, name: existing?.name ?? f.name }))
                }}
              >
                <SelectTrigger className="h-8 text-xs">
                  <SelectValue placeholder="Choose a known topic…" />
                </SelectTrigger>
                <SelectContent>
                  {namedAll.map((t) => (
                    <SelectItem key={t.thread_id} value={String(t.thread_id)} className="text-xs">
                      {t.name}
                      <span className="ml-2 font-mono text-muted-foreground">#{t.thread_id}</span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
            <ManualThreadInput
              value={form.linkOrId}
              onChange={(v) => setForm((f) => ({ ...f, linkOrId: v }))}
              onEnter={() => parsedId !== null && addThread()}
              showHint={namedAll.length === 0}
            />
          </div>

          <div className="space-y-1.5">
            <p className="text-xs font-medium">
              Name <span className="font-normal text-muted-foreground">(optional)</span>
            </p>
            <Input
              className="h-7 text-xs"
              placeholder="e.g. General, News, Off-topic"
              value={form.name}
              onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))}
              onKeyDown={(e) => e.key === 'Enter' && parsedId !== null && addThread()}
            />
            <p className="text-xs text-muted-foreground">
              Label shown here. If left blank and the bot has seen this topic created,
              the name will be filled automatically.
            </p>
          </div>

          <div className="flex items-center gap-3">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => setForm((f) => ({ ...f, enabled: !f.enabled }))}
              className={cn(
                'h-7 px-2.5 text-xs',
                form.enabled
                  ? 'border-green-500/40 bg-green-500/10 text-green-700 dark:text-green-400'
                  : 'border-destructive/40 bg-destructive/10 text-destructive'
              )}
            >
              {form.enabled ? 'Allow' : 'Deny'}
            </Button>
            <p className="text-xs text-muted-foreground">access in this thread</p>
          </div>

          <div className="flex gap-2">
            <Button size="sm" className="h-7 text-xs" disabled={parsedId === null} onClick={addThread}>
              Save
            </Button>
            <Button size="sm" variant="ghost" className="h-7 text-xs" onClick={closeAdd}>
              Cancel
            </Button>
          </div>
        </div>
      )}
    </div>
  )
}

export default function AdminGroupsPage() {
  const { t } = useTranslation()
  const [items, setItems] = useState<Group[]>([])
  const [total, setTotal] = useState(0)
  const [loading, setLoading] = useState(true)
  const [edit, setEdit] = useState<EditState | null>(null)
  const [saving, setSaving] = useState(false)
  const [expanded, setExpanded] = useState<number | null>(null)

  const load = () => {
    setLoading(true)
    apiClient
      .get<Group[]>('/groups')
      .then((res) => { setItems(res.data); setTotal(res.data.length) })
      .catch(() => toast.error(t('common.load_error')))
      .finally(() => setLoading(false))
  }

  useEffect(() => { load() }, [])

  const save = async () => {
    if (!edit) return
    setSaving(true)
    try {
      if (edit.isNew) {
        const body: GroupCreateRequest = {
          id: parseInt(edit.newId, 10),
          title: edit.title || undefined,
          enabled: edit.enabled,
          auto_grant_role: edit.auto_grant_role,
          allow_pm: edit.allow_pm,
          nsfw_allowed: edit.nsfw_allowed,
        }
        await apiClient.post('/groups', body)
        toast.success(t('common.save'))
      } else if (edit.group) {
        const body: GroupUpdateRequest = {
          title: edit.title || undefined,
          enabled: edit.enabled,
          auto_grant_role: edit.auto_grant_role,
          allow_pm: edit.allow_pm,
          nsfw_allowed: edit.nsfw_allowed,
          storage_chat_id: edit.storage_chat_id ? parseInt(edit.storage_chat_id, 10) : null,
          storage_thread_id: edit.storage_thread_id ? parseInt(edit.storage_thread_id, 10) : null,
        }
        await apiClient.patch(`/groups/${edit.group.id}`, body)
        toast.success(t('common.save'))
      }
      setEdit(null)
      load()
    } catch {
      toast.error(t('common.load_error'))
    } finally {
      setSaving(false)
    }
  }

  return (
    <TooltipProvider delayDuration={300}>
      <div className="space-y-4">
        <Card>
          <CardHeader className="px-4 py-3">
            <div className="flex items-center justify-between gap-2">
              <CardTitle className="flex items-center gap-2 text-base">
                <MessageSquare className="h-4 w-4 text-muted-foreground" />
                {loading
                  ? t('groups.title', { defaultValue: 'Groups' })
                  : `${total} group${total !== 1 ? 's' : ''}`}
              </CardTitle>
              <Button size="sm" onClick={() => setEdit(defaultEdit())}>{t('groups.add_group')}</Button>
            </div>
          </CardHeader>

          <CardContent className="p-0">
            {loading ? (
              <div className="divide-y divide-border px-3 py-1">
                {Array.from({ length: 3 }).map((_, i) => (
                  <div key={i} className="flex items-center gap-3 py-2.5">
                    <Skeleton className="size-8 rounded-md shrink-0" />
                    <div className="flex-1 space-y-1.5">
                      <Skeleton className="h-3.5 w-36" />
                      <Skeleton className="h-3 w-24" />
                    </div>
                    <Skeleton className="h-5 w-14" />
                  </div>
                ))}
              </div>
            ) : items.length === 0 ? (
              <div className="flex justify-center py-12 text-muted-foreground text-sm">{t('groups.no_groups')}</div>
            ) : (
              <div className="divide-y divide-border px-3 py-1">
                {items.map((group) => (
                  <div key={group.id}>
                    <Item
                      size="sm"
                      className="py-2.5 rounded-none border-0 cursor-pointer"
                      onClick={() => setExpanded((p) => p === group.id ? null : group.id)}
                    >
                      <ItemMedia
                        variant="icon"
                        className={cn(
                          'size-8 rounded-md',
                          group.enabled ? 'bg-green-500/10 text-green-600' : 'bg-muted text-muted-foreground'
                        )}
                      >
                        <MessageSquare className="size-4" />
                      </ItemMedia>
                      <ItemContent>
                        <ItemTitle>
                          {group.title ?? <span className="text-muted-foreground italic">{t('groups.untitled')}</span>}
                        </ItemTitle>
                        <ItemDescription>
                          <span className="font-mono text-[11px]">{group.id}</span>
                          {group.nsfw_allowed && <span className="text-amber-500"> · NSFW</span>}
                        </ItemDescription>
                      </ItemContent>
                      <ItemActions>
                        {group.enabled
                          ? <SuccessBadge>{t('groups.active')}</SuccessBadge>
                          : <Badge variant="outline">{t('groups.disabled')}</Badge>}
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <Button
                              variant="ghost"
                              size="icon"
                              className="h-8 w-8"
                              onClick={(e) => { e.stopPropagation(); setEdit(defaultEdit(group)) }}
                            >
                              <Pencil className="h-3.5 w-3.5" />
                            </Button>
                          </TooltipTrigger>
                          <TooltipContent>{t('common.edit')}</TooltipContent>
                        </Tooltip>
                      </ItemActions>
                    </Item>
                    {expanded === group.id && <ThreadRows groupId={group.id} />}
                  </div>
                ))}
              </div>
            )}
          </CardContent>
        </Card>

        <Dialog open={!!edit} onOpenChange={(open: boolean) => !open && setEdit(null)}>
          <DialogContent className="sm:max-w-md">
            <DialogHeader>
              <DialogTitle>{edit?.isNew ? t('groups.edit_title_new') : t('groups.edit_title_edit')}</DialogTitle>
            </DialogHeader>

            {edit && (
              <div className="space-y-5">
                {edit.isNew && (
                  <div className="space-y-1.5">
                    <Label htmlFor="group-id">{t('groups.field_chat_id')}</Label>
                    <Input id="group-id" type="number" placeholder="-100123456789"
                      value={edit.newId} onChange={(e) => setEdit({ ...edit, newId: e.target.value })} />
                  </div>
                )}

                <div className="grid grid-cols-[1fr_auto] gap-x-3 gap-y-1.5 items-center">
                  <div className="space-y-1.5">
                    <Label htmlFor="group-title">{t('groups.field_title')}</Label>
                    <Input id="group-title" value={edit.title} onChange={(e) => setEdit({ ...edit, title: e.target.value })} />
                  </div>
                  <div className="space-y-1.5">
                    <Label className="text-muted-foreground">{t('groups.col_status')}</Label>
                    <div className="flex items-center h-9 px-1">
                      <Switch
                        id="group-enabled"
                        checked={edit.enabled}
                        onCheckedChange={(checked: boolean) => setEdit({ ...edit, enabled: checked })}
                      />
                    </div>
                  </div>
                </div>

                <div className="rounded-md border divide-y">
                  <div className="flex items-center justify-between px-3 py-2.5">
                    <Label htmlFor="group-auto-role" className="font-normal">{t('groups.field_auto_role')}</Label>
                    <Select value={edit.auto_grant_role} onValueChange={(v: string) => setEdit({ ...edit, auto_grant_role: v as UserRole })}>
                      <SelectTrigger id="group-auto-role" className="h-8 w-28"><SelectValue /></SelectTrigger>
                      <SelectContent>
                        {ROLES.map((r) => <SelectItem key={r} value={r}>{r}</SelectItem>)}
                      </SelectContent>
                    </Select>
                  </div>
                  <div className="flex items-center justify-between px-3 py-2.5">
                    <div>
                      <Label htmlFor="group-allow-pm" className="font-normal">{t('groups.field_allow_pm')}</Label>
                    </div>
                    <Switch
                      id="group-allow-pm"
                      checked={edit.allow_pm}
                      onCheckedChange={(checked: boolean) => setEdit({ ...edit, allow_pm: checked })}
                    />
                  </div>
                  <div className="flex items-center justify-between px-3 py-2.5">
                    <div>
                      <Label htmlFor="group-nsfw" className="font-normal">{t('groups.field_nsfw')}</Label>
                      <p className="text-xs text-muted-foreground">{t('groups.field_nsfw_hint')}</p>
                    </div>
                    <Switch
                      id="group-nsfw"
                      checked={edit.nsfw_allowed}
                      onCheckedChange={(checked: boolean) => setEdit({ ...edit, nsfw_allowed: checked })}
                    />
                  </div>
                </div>

                {!edit.isNew && (
                  <div className="space-y-3">
                    <div>
                      <p className="text-sm font-medium">{t('groups.inline_storage')}</p>
                      <p className="text-xs text-muted-foreground">{t('groups.inline_storage_hint')}</p>
                    </div>
                    <div className="grid grid-cols-2 gap-2">
                      <div className="space-y-1.5">
                        <Label htmlFor="storage-chat" className="text-xs">{t('groups.storage_chat_id')}</Label>
                        <Input
                          id="storage-chat"
                          type="number"
                          placeholder="-100…"
                          value={edit.storage_chat_id}
                          onChange={(e) => setEdit({ ...edit, storage_chat_id: e.target.value })}
                        />
                      </div>
                      <div className="space-y-1.5">
                        <Label htmlFor="storage-thread" className="text-xs">{t('groups.storage_thread_id')}</Label>
                        <Input
                          id="storage-thread"
                          type="number"
                          placeholder="Thread ID"
                          value={edit.storage_thread_id}
                          onChange={(e) => setEdit({ ...edit, storage_thread_id: e.target.value })}
                        />
                      </div>
                    </div>
                  </div>
                )}
              </div>
            )}

            <DialogFooter className="flex-row gap-2 sm:space-x-0">
              <Button variant="outline" className="flex-1" onClick={() => setEdit(null)}>{t('common.cancel')}</Button>
              <Button className="flex-1" onClick={save} disabled={saving}>{saving ? t('common.loading') : t('common.save')}</Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </div>
    </TooltipProvider>
  )
}
