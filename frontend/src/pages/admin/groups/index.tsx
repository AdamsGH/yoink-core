import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { ChevronDown, ChevronRight, Plus, Trash2 } from 'lucide-react'

import { apiClient } from '@core/lib/api-client'
import { cn, formatDate } from '@core/lib/utils'
import type { Group, GroupCreateRequest, GroupUpdateRequest, ThreadPolicy, UserRole } from '@core/types/api'
import { Badge } from '@core/components/ui/badge'
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
import { Label } from '@core/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@core/components/ui/select'
import { Switch } from '@core/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@core/components/ui/table'
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

/**
 * Parse thread_id from a Telegram message link.
 *
 * Private group links look like:
 *   https://t.me/c/1197008640/24/40    - channel_id / thread_id / message_id
 *   https://t.me/c/1197008640/40       - channel_id / message_id (no thread = main chat)
 *
 * The second segment is thread_id only when there are THREE numeric path parts.
 * With two parts it's just a direct message link (no thread context).
 *
 * Returns null if input is not a recognisable link or plain number.
 */
function parseThreadId(input: string): number | null {
  const trimmed = input.trim()

  // Plain number
  if (/^\d+$/.test(trimmed)) return parseInt(trimmed, 10)

  // t.me/c/{channel}/{thread}/{message}
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
  linkOrId: string   // raw input: message link or plain number
  name: string       // display name
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

  useEffect(() => { load() }, [groupId]) // eslint-disable-line react-hooks/exhaustive-deps

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
        <span className="text-xs font-semibold text-muted-foreground uppercase tracking-wide">{t('groups.title').replace('s','s')}</span>
        {!adding && (
          <Button size="sm" variant="ghost" className="h-6 gap-1 text-xs" onClick={openAdd}>
            <Plus className="h-3 w-3" /> Add policy
          </Button>
        )}
      </div>

      {threads.length === 0 && !adding && (
        <p className="text-xs text-muted-foreground">
          {t('groups.thread_policies')}
          from service messages when topics are created.
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
            <Badge variant={t.enabled ? 'success' : 'outline'} className="shrink-0 text-xs">
              {t.enabled ? 'allowed' : 'denied'}
            </Badge>
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

          {/* Topic source: select from known or paste link */}
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

          {/* Display name */}
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

          {/* Allow / Deny toggle */}
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
    <div className="space-y-5">
      <Card>
        <CardHeader className="flex flex-row items-center justify-between px-4 py-3 space-y-0">
          <CardTitle className="text-sm font-medium">{total} group{total !== 1 ? 's' : ''}</CardTitle>
          <Button size="sm" onClick={() => setEdit(defaultEdit())}>{t('groups.add_group')}</Button>
        </CardHeader>
        <CardContent className="p-0">
          {loading ? (
            <div className="flex justify-center py-12 text-muted-foreground">{t('common.loading')}</div>
          ) : items.length === 0 ? (
            <div className="flex justify-center py-12 text-muted-foreground">{t('groups.no_groups')} configured</div>
          ) : (
            <>
              {/* Desktop table */}
              <div className="hidden md:block">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead className="w-8" />
                      <TableHead>{t('groups.col_group')}</TableHead>
                      <TableHead>{t('groups.col_status')}</TableHead>
                      <TableHead>{t('groups.col_role')}</TableHead>
                      <TableHead>{t('groups.col_pm')}</TableHead>
                      <TableHead>NSFW</TableHead>
                      <TableHead>{t('groups.col_added')}</TableHead>
                      <TableHead />
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {items.map((group) => (
                      <>
                        <TableRow key={group.id} className="cursor-pointer">
                          <TableCell onClick={() => setExpanded((p) => p === group.id ? null : group.id)} className="text-muted-foreground">
                            {expanded === group.id ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />}
                          </TableCell>
                          <TableCell>
                            <p className="text-sm font-medium">{group.title ?? <span className="text-muted-foreground italic">{t('groups.untitled')}</span>}</p>
                            <p className="font-mono text-xs text-muted-foreground">{group.id}</p>
                          </TableCell>
                          <TableCell>
                            <Badge variant={group.enabled ? 'success' : 'outline'}>
                              {group.enabled ? t('groups.active') : t('groups.disabled')}
                            </Badge>
                          </TableCell>
                          <TableCell><Badge variant="secondary">{group.auto_grant_role}</Badge></TableCell>
                          <TableCell>
                            <Badge variant={group.allow_pm ? 'success' : 'outline'}>
                              {group.allow_pm ? 'yes' : 'no'}
                            </Badge>
                          </TableCell>
                          <TableCell>
                            <Badge variant={group.nsfw_allowed ? 'warning' : 'outline'}>
                              {group.nsfw_allowed ? t('groups.allowed') : t('groups.blocked')}
                            </Badge>
                          </TableCell>
                          <TableCell className="text-xs text-muted-foreground">{formatDate(group.created_at)}</TableCell>
                          <TableCell>
                            <Button variant="ghost" size="sm" onClick={() => setEdit(defaultEdit(group))}>{t('common.edit')}</Button>
                          </TableCell>
                        </TableRow>
                        {expanded === group.id && (
                          <TableRow key={`threads-${group.id}`} className="hover:bg-transparent">
                            <TableCell colSpan={8} className="p-0">
                              <ThreadRows groupId={group.id} />
                            </TableCell>
                          </TableRow>
                        )}
                      </>
                    ))}
                  </TableBody>
                </Table>
              </div>

              {/* Mobile cards */}
              <div className="md:hidden divide-y divide-border">
                {items.map((group) => (
                  <div key={group.id} className="px-4 py-3 space-y-2">
                    <div className="flex items-start justify-between gap-2">
                      <div>
                        <p className="text-sm font-medium">{group.title ?? <span className="text-muted-foreground">{t('groups.untitled')}</span>}</p>
                        <p className="font-mono text-xs text-muted-foreground">{group.id}</p>
                      </div>
                      <Button variant="ghost" size="sm" className="h-7 text-xs shrink-0" onClick={() => setEdit(defaultEdit(group))}>{t('common.edit')}</Button>
                    </div>
                    <div className="flex flex-wrap gap-1.5">
                      <Badge variant={group.enabled ? 'success' : 'outline'}>{group.enabled ? t('groups.active') : t('groups.disabled')}</Badge>
                      <Badge variant="secondary">{group.auto_grant_role}</Badge>
                      <Badge variant={group.allow_pm ? 'success' : 'outline'}>{`PM: ${group.allow_pm ? t('common.yes') : t('common.no')}`}</Badge>
                      <Badge variant={group.nsfw_allowed ? 'warning' : 'outline'}>{`NSFW: ${group.nsfw_allowed ? t('common.yes') : t('common.no')}`}</Badge>
                    </div>
                    <Button
                      variant="ghost"
                      size="sm"
                      className="h-auto px-0 py-0 text-xs text-muted-foreground hover:bg-transparent hover:text-foreground"
                      onClick={() => setExpanded((p) => p === group.id ? null : group.id)}
                    >
                      {expanded === group.id ? <ChevronDown className="h-3 w-3" /> : <ChevronRight className="h-3 w-3" />}
                      {t('groups.thread_policies')}
                    </Button>
                    {expanded === group.id && <ThreadRows groupId={group.id} />}
                  </div>
                ))}
              </div>
            </>
          )}
        </CardContent>
      </Card>

      <Dialog open={!!edit} onOpenChange={(open: boolean) => !open && setEdit(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{edit?.isNew ? t('groups.edit_title_new') : t('groups.edit_title_edit')}</DialogTitle>
          </DialogHeader>

          {edit && (
            <div className="space-y-4">
              {edit.isNew && (
                <div className="space-y-1.5">
                  <Label htmlFor="group-id">{t('groups.field_chat_id')}</Label>
                  <Input id="group-id" type="number" placeholder="-100123456789"
                    value={edit.newId} onChange={(e) => setEdit({ ...edit, newId: e.target.value })} />
                </div>
              )}

              <div className="flex items-center gap-3">
                <Switch
                  id="group-enabled"
                  checked={edit.enabled}
                  onCheckedChange={(checked: boolean) => setEdit({ ...edit, enabled: checked })}
                />
                <div>
                  <Label htmlFor="group-enabled">{t('groups.field_active')}</Label>
                  <p className="text-xs text-muted-foreground">{t('groups.field_active_hint')}</p>
                </div>
              </div>

              <div className="space-y-1.5">
                <Label htmlFor="group-title">{t('groups.field_title')}</Label>
                <Input id="group-title" value={edit.title} onChange={(e) => setEdit({ ...edit, title: e.target.value })} />
              </div>

              <div className="space-y-1.5">
                <Label>{t('groups.field_auto_role')}</Label>
                <Select value={edit.auto_grant_role} onValueChange={(v: string) => setEdit({ ...edit, auto_grant_role: v as UserRole })}>
                  <SelectTrigger><SelectValue /></SelectTrigger>
                  <SelectContent>
                    {ROLES.map((r) => <SelectItem key={r} value={r}>{r}</SelectItem>)}
                  </SelectContent>
                </Select>
              </div>

              <div className="flex items-center gap-3">
                <Switch
                  id="group-allow-pm"
                  checked={edit.allow_pm}
                  onCheckedChange={(checked: boolean) => setEdit({ ...edit, allow_pm: checked })}
                />
                <Label htmlFor="group-allow-pm">{t('groups.field_allow_pm')}</Label>
              </div>

              <div className="flex items-center gap-3">
                <Switch
                  id="group-nsfw"
                  checked={edit.nsfw_allowed}
                  onCheckedChange={(checked: boolean) => setEdit({ ...edit, nsfw_allowed: checked })}
                />
                <div>
                  <Label htmlFor="group-nsfw">{t('groups.field_nsfw')}</Label>
                  <p className="text-xs text-muted-foreground">{t('groups.field_nsfw_hint')}</p>
                </div>
              </div>

              {!edit.isNew && (
                <div className="space-y-3 border-t pt-3">
                  <div>
                    <p className="text-sm font-medium">{t('groups.inline_storage')}</p>
                    <p className="text-xs text-muted-foreground">
                      {t('groups.inline_storage_hint')}
                    </p>
                  </div>
                  <div className="space-y-1.5">
                    <Label htmlFor="storage-chat">{t('groups.storage_chat_id')}</Label>
                    <Input
                      id="storage-chat"
                      type="number"
                      placeholder="-100123456789"
                      value={edit.storage_chat_id}
                      onChange={(e) => setEdit({ ...edit, storage_chat_id: e.target.value })}
                    />
                  </div>
                  <div className="space-y-1.5">
                    <Label htmlFor="storage-thread">{t('groups.storage_thread_id')}</Label>
                    <Input
                      id="storage-thread"
                      type="number"
                      placeholder="Thread / topic ID"
                      value={edit.storage_thread_id}
                      onChange={(e) => setEdit({ ...edit, storage_thread_id: e.target.value })}
                    />
                  </div>
                </div>
              )}
            </div>
          )}

          <DialogFooter>
            <Button variant="outline" onClick={() => setEdit(null)}>{t('common.cancel')}</Button>
            <Button onClick={save} disabled={saving}>{saving ? t('common.loading') : t('common.save')}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
