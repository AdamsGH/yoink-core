import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { MessageSquare, Pencil, Plus, RefreshCw, Settings2, Trash2 } from 'lucide-react'

import { apiClient } from '@core/lib/api-client'
import { cn } from '@core/lib/utils'
import type { Group, ThreadPolicy, UserRole } from '@core/types/api'
import { useAdminGroups } from './useAdminGroups'
import type { EditState } from './useAdminGroups'
import { Avatar, AvatarFallback, AvatarImage, Badge, Button, Card, CardContent, CardHeader, CardTitle, Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, Input, Item, ItemActions, ItemContent, ItemDescription, ItemMedia, ItemTitle, Label, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Skeleton, Switch, Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@ui'
import { InlineSelect, SuccessBadge } from '@app'
import { toast } from '@core/components/ui/toast'

const ROLES: UserRole[] = ['owner', 'admin', 'moderator', 'user', 'restricted', 'banned']

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

function AddThreadDialog({
  open,
  onClose,
  groupId,
  threads,
  onDone,
}: {
  open: boolean
  onClose: () => void
  groupId: number
  threads: ThreadPolicy[]
  onDone: () => void
}) {
  const { t } = useTranslation()
  const [form, setForm] = useState<AddState>(DEFAULT_ADD)
  const [saving, setSaving] = useState(false)

  const namedAll = threads.filter((tp) => tp.name && tp.thread_id != null)
  const parsedId = parseThreadId(form.linkOrId)

  useEffect(() => {
    if (open) setForm(DEFAULT_ADD)
  }, [open])

  const handleSave = async () => {
    if (parsedId === null) return
    setSaving(true)
    try {
      await apiClient.post(`/groups/${groupId}/threads`, {
        thread_id: parsedId,
        name: form.name.trim() || null,
        enabled: form.enabled,
      })
      toast.success('Thread policy saved')
      onClose()
      onDone()
    } catch {
      toast.error(t('common.load_error'))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{t('groups.add_thread_title', { defaultValue: 'Add thread policy' })}</DialogTitle>
          <DialogDescription>
            {t('groups.add_thread_hint', { defaultValue: 'Select a known topic or paste a message link to identify the thread.' })}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          <div className="space-y-1.5">
            <Label>{t('groups.thread_id_label', { defaultValue: 'Thread' })}</Label>
            {namedAll.length > 0 && (
              <InlineSelect
                options={namedAll.map((tp) => ({ value: String(tp.thread_id), label: tp.name ?? String(tp.thread_id), meta: `#${tp.thread_id}` }))}
                value={form.linkOrId}
                onValueChange={(v, opt) => setForm((f) => ({ ...f, linkOrId: v, name: opt.label }))}
                placeholder={t('groups.choose_topic', { defaultValue: 'Choose a known topic...' })}
                searchPlaceholder="Search topics..."
              />
            )}
            <ManualThreadInput
              value={form.linkOrId}
              onChange={(v) => setForm((f) => ({ ...f, linkOrId: v }))}
              onEnter={() => parsedId !== null && handleSave()}
              showHint={namedAll.length === 0}
            />
          </div>

          <div className="space-y-1.5">
            <Label>
              {t('groups.thread_name_label', { defaultValue: 'Name' })}
              <span className="font-normal text-muted-foreground ml-1">({t('common.optional', { defaultValue: 'optional' })})</span>
            </Label>
            <Input
              placeholder="e.g. General, News, Off-topic"
              value={form.name}
              onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))}
              onKeyDown={(e) => e.key === 'Enter' && parsedId !== null && handleSave()}
            />
          </div>

          <div className="flex items-center justify-between rounded-md border px-3 py-2.5">
            <Label htmlFor="thread-access" className="font-normal">
              {t('groups.thread_access_label', { defaultValue: 'Allow access' })}
            </Label>
            <Switch
              id="thread-access"
              checked={form.enabled}
              onCheckedChange={(v) => setForm((f) => ({ ...f, enabled: v }))}
            />
          </div>
        </div>

        <DialogFooter className="flex-row gap-2 sm:space-x-0">
          <Button variant="outline" className="flex-1" onClick={onClose}>{t('common.cancel')}</Button>
          <Button className="flex-1" onClick={handleSave} disabled={parsedId === null || saving}>
            {saving ? t('common.loading') : t('common.save')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function ThreadPoliciesDialog({
  open,
  onClose,
  group,
}: {
  open: boolean
  onClose: () => void
  group: Group
}) {
  const { t } = useTranslation()
  const [threads, setThreads] = useState<ThreadPolicy[]>([])
  const [loading, setLoading] = useState(true)
  const [adding, setAdding] = useState(false)
  const [sessionAvailable, setSessionAvailable] = useState(false)
  const [scanning, setScanning] = useState(false)
  const sessionChecked = useRef(false)

  const load = () => {
    setLoading(true)
    apiClient
      .get<ThreadPolicy[]>(`/groups/${group.id}/threads`)
      .then((res) => setThreads(res.data))
      .catch(() => toast.error(t('common.load_error')))
      .finally(() => setLoading(false))
  }

  useEffect(() => {
    if (!open) return
    load()
    if (!sessionChecked.current) {
      sessionChecked.current = true
      apiClient.get<{ available: boolean }>('/threads/status')
        .then((res) => setSessionAvailable(res.data.available))
        .catch(() => {})
    }
  }, [open])

  const toggle = async (policy: ThreadPolicy) => {
    setThreads((prev) => prev.map((tp) => tp.id === policy.id ? { ...tp, enabled: !tp.enabled } : tp))
    try {
      await apiClient.post(`/groups/${group.id}/threads`, { thread_id: policy.thread_id, name: policy.name, enabled: !policy.enabled })
    } catch {
      setThreads((prev) => prev.map((tp) => tp.id === policy.id ? { ...tp, enabled: policy.enabled } : tp))
      toast.error(t('common.load_error'))
    }
  }

  const remove = async (policy: ThreadPolicy) => {
    if (!confirm(`Remove policy for "${threadLabel(policy)}"?`)) return
    try {
      await apiClient.delete(`/groups/${group.id}/threads/${policy.id}`)
      load()
    } catch { toast.error(t('common.load_error')) }
  }

  const scan = async () => {
    setScanning(true)
    try {
      const res = await apiClient.post<{ total_count: number; upserted: number }>(`/threads/scan/${group.id}`)
      toast.success(`Synced ${res.data.upserted} of ${res.data.total_count} topics`)
      load()
    } catch {
      toast.error(t('common.load_error'))
    } finally {
      setScanning(false)
    }
  }

  return (
    <>
      <Dialog open={open && !adding} onOpenChange={(o) => !o && onClose()}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>{t('groups.thread_policies')}</DialogTitle>
            <DialogDescription>
              {group.title ?? group.id}
            </DialogDescription>
          </DialogHeader>

          {loading ? (
            <div className="space-y-2 py-2">
              {[1, 2].map((i) => (
                <div key={i} className="flex items-center gap-3">
                  <Skeleton className="h-4 flex-1" />
                  <Skeleton className="h-5 w-14" />
                </div>
              ))}
            </div>
          ) : threads.length === 0 ? (
            <div className="py-6 text-center text-sm text-muted-foreground">
              {t('groups.no_thread_policies', { defaultValue: 'No thread policies configured.' })}
            </div>
          ) : (
            <div className="divide-y divide-border rounded-md border max-h-72 overflow-y-auto">
              {threads.map((tp) => (
                <div key={tp.id} className="flex items-center gap-2 px-3 py-2.5">
                  <div className="min-w-0 flex-1">
                    <p className="text-sm font-medium">{threadLabel(tp)}</p>
                    {tp.name && tp.thread_id != null && (
                      <p className="font-mono text-xs text-muted-foreground">#{tp.thread_id}</p>
                    )}
                  </div>
                  <Switch
                    checked={tp.enabled}
                    onCheckedChange={() => toggle(tp)}
                  />
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button variant="ghost" size="icon" className="h-7 w-7 text-muted-foreground hover:text-destructive" onClick={() => remove(tp)}>
                        <Trash2 className="h-3.5 w-3.5" />
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>{t('common.delete')}</TooltipContent>
                  </Tooltip>
                </div>
              ))}
            </div>
          )}

          <DialogFooter className="flex-row gap-2 sm:space-x-0">
            {sessionAvailable && (
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button variant="outline" size="icon" className="h-9 w-9 shrink-0" onClick={scan} disabled={scanning}>
                    <RefreshCw className={cn('h-4 w-4', scanning && 'animate-spin')} />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>{t('groups.scan_topics', { defaultValue: 'Sync topics from Telegram' })}</TooltipContent>
              </Tooltip>
            )}
            <Button variant="outline" className="flex-1" onClick={() => setAdding(true)}>
              <Plus className="h-4 w-4 mr-2" />
              {t('groups.add_thread_title', { defaultValue: 'Add policy' })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <AddThreadDialog
        open={adding}
        onClose={() => setAdding(false)}
        groupId={group.id}
        threads={threads}
        onDone={() => { setAdding(false); load() }}
      />
    </>
  )
}

function GroupCard({
  group,
  onEdit,
}: {
  group: Group
  onEdit: () => void
}) {
  const { t } = useTranslation()
  const [threadCount, setThreadCount] = useState<number | null>(null)
  const [showThreads, setShowThreads] = useState(false)

  useEffect(() => {
    apiClient
      .get<ThreadPolicy[]>(`/groups/${group.id}/threads`)
      .then((res) => setThreadCount(res.data.length))
      .catch(() => {})
  }, [group.id])

  return (
    <>
      <div className="px-3 py-1">
        <Item size="sm" className="py-2.5 rounded-none border-0">
          <ItemMedia variant="icon" className="size-8 shrink-0">
            <Avatar className={cn('size-8 rounded-md', !group.photo_url && (group.enabled ? 'bg-green-500/10' : 'bg-muted'))}>
              <AvatarImage src={`${apiClient.defaults.baseURL}/groups/${group.id}/photo`} className="rounded-md object-cover" />
              <AvatarFallback className={cn('size-8 rounded-md text-xs font-medium', group.enabled ? 'text-green-600' : 'text-muted-foreground')}>
                <MessageSquare className="size-4" />
              </AvatarFallback>
            </Avatar>
          </ItemMedia>
          <ItemContent className="gap-0">
            <ItemTitle className="leading-snug">
              {group.title ?? <span className="text-muted-foreground italic">{t('groups.untitled')}</span>}
            </ItemTitle>
            <ItemDescription className="mt-0 leading-snug">
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
                <Button variant="ghost" size="icon" className="relative h-8 w-8" onClick={() => setShowThreads(true)}>
                  <Settings2 className="h-3.5 w-3.5" />
                  {threadCount !== null && threadCount > 0 && (
                    <span className="absolute -top-0.5 -right-0.5 flex h-3.5 w-3.5 items-center justify-center rounded-full bg-muted text-[9px] font-medium tabular-nums text-muted-foreground">
                      {threadCount}
                    </span>
                  )}
                </Button>
              </TooltipTrigger>
              <TooltipContent>{t('groups.thread_policies')}</TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="ghost" size="icon" className="h-8 w-8" onClick={onEdit}>
                  <Pencil className="h-3.5 w-3.5" />
                </Button>
              </TooltipTrigger>
              <TooltipContent>{t('common.edit')}</TooltipContent>
            </Tooltip>
          </ItemActions>
        </Item>
      </div>

      <ThreadPoliciesDialog
        open={showThreads}
        onClose={() => {
          setShowThreads(false)
          apiClient.get<ThreadPolicy[]>(`/groups/${group.id}/threads`)
            .then((res) => setThreadCount(res.data.length))
            .catch(() => {})
        }}
        group={group}
      />
    </>
  )
}

export default function AdminGroupsPage() {
  const { t } = useTranslation()
  const { items, total, loading, edit, setEdit, saving, save } = useAdminGroups()

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
              <Button size="sm" className="h-7 px-2.5 text-xs" onClick={() => setEdit(defaultEdit())}>{t('groups.add_group')}</Button>
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
              <div className="divide-y divide-border">
                {items.map((group) => (
                  <GroupCard
                    key={group.id}
                    group={group}
                    onEdit={() => setEdit(defaultEdit(group))}
                  />
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
                    <Input id="group-id" inputMode="numeric" placeholder="-100123456789"
                      className="[appearance:textfield] [&::-webkit-outer-spin-button]:appearance-none [&::-webkit-inner-spin-button]:appearance-none"
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
                          inputMode="numeric"
                          placeholder="-100…"
                          className="[appearance:textfield] [&::-webkit-outer-spin-button]:appearance-none [&::-webkit-inner-spin-button]:appearance-none"
                          value={edit.storage_chat_id}
                          onChange={(e) => setEdit({ ...edit, storage_chat_id: e.target.value })}
                        />
                      </div>
                      <div className="space-y-1.5">
                        <Label htmlFor="storage-thread" className="text-xs">{t('groups.storage_thread_id')}</Label>
                        <Input
                          id="storage-thread"
                          inputMode="numeric"
                          placeholder="Thread ID"
                          className="[appearance:textfield] [&::-webkit-outer-spin-button]:appearance-none [&::-webkit-inner-spin-button]:appearance-none"
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
