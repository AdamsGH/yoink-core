import { Pencil, Plus, Trash2 } from 'lucide-react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'

import {
  Badge,
  Button,
  Dialog,
  DialogActions,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DividedList,
  Input,
  JsonEditor,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  SkeletonList,
  Switch,
} from '@ui'
import { EmptyState, PageContainer } from '@app'

import { createRule, deleteRule, listRules, updateRule } from '@inbox/api/rules'
import type { InboxRule, RuleTrigger } from '@inbox/types'

const TRIGGER_OPTIONS: { value: RuleTrigger; label: string }[] = [
  { value: 'item_ingested', label: 'Item ingested' },
  { value: 'item_classified', label: 'Item classified' },
  { value: 'star_synced', label: 'Star synced' },
]

const CONDITION_PLACEHOLDER = JSON.stringify(
  [{ field: 'kind', op: 'eq', value: 'github_repo' }],
  null,
  2,
)

const ACTION_PLACEHOLDER = JSON.stringify(
  [{ type: 'add_category', params: { category_name: 'Dev' } }],
  null,
  2,
)

interface FormState {
  name: string
  enabled: boolean
  priority: string
  trigger: RuleTrigger
  conditionsJson: string
  actionsJson: string
}

function emptyForm(): FormState {
  return {
    name: '',
    enabled: true,
    priority: '100',
    trigger: 'item_classified',
    conditionsJson: CONDITION_PLACEHOLDER,
    actionsJson: ACTION_PLACEHOLDER,
  }
}

function ruleToForm(r: InboxRule): FormState {
  return {
    name: r.name,
    enabled: r.enabled,
    priority: String(r.priority),
    trigger: r.trigger,
    conditionsJson: JSON.stringify(r.conditions ?? [], null, 2),
    actionsJson: JSON.stringify(r.actions ?? [], null, 2),
  }
}

function parseJson(raw: string): object[] | null {
  try {
    const parsed = JSON.parse(raw)
    if (Array.isArray(parsed)) return parsed
    return null
  } catch {
    return null
  }
}

export default function RulesPage() {
  const [rules, setRules] = useState<InboxRule[]>([])
  const [loading, setLoading] = useState(true)
  const [open, setOpen] = useState(false)
  const [editing, setEditing] = useState<InboxRule | null>(null)
  const [form, setForm] = useState<FormState>(emptyForm())
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    listRules()
      .then(setRules)
      .catch((err) => {
        if (import.meta.env.DEV) console.error(err)
        toast.error('Failed to load rules')
      })
      .finally(() => setLoading(false))
  }, [])

  function openCreate() {
    setEditing(null)
    setForm(emptyForm())
    setOpen(true)
  }

  function openEdit(rule: InboxRule) {
    setEditing(rule)
    setForm(ruleToForm(rule))
    setOpen(true)
  }

  async function onToggle(rule: InboxRule) {
    try {
      const updated = await updateRule(rule.id, { enabled: !rule.enabled })
      setRules((prev) => prev.map((r) => (r.id === updated.id ? updated : r)))
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to update rule')
    }
  }

  async function onDelete(id: number) {
    try {
      await deleteRule(id)
      setRules((prev) => prev.filter((r) => r.id !== id))
      toast.success('Rule deleted')
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to delete rule')
    }
  }

  async function onSave() {
    const conditions = parseJson(form.conditionsJson)
    const actions = parseJson(form.actionsJson)
    if (conditions === null) {
      toast.error('Conditions must be a valid JSON array')
      return
    }
    if (actions === null) {
      toast.error('Actions must be a valid JSON array')
      return
    }
    const priority = parseInt(form.priority, 10)
    if (isNaN(priority) || priority < 0 || priority > 9999) {
      toast.error('Priority must be 0-9999')
      return
    }

    setSaving(true)
    try {
      if (editing) {
        const updated = await updateRule(editing.id, {
          name: form.name,
          enabled: form.enabled,
          priority,
          trigger: form.trigger,
          conditions: conditions as never,
          actions: actions as never,
        })
        setRules((prev) => prev.map((r) => (r.id === updated.id ? updated : r)))
        toast.success('Rule updated')
      } else {
        const created = await createRule({
          name: form.name,
          enabled: form.enabled,
          priority,
          trigger: form.trigger,
          conditions: conditions as never,
          actions: actions as never,
        })
        setRules((prev) => [...prev, created])
        toast.success('Rule created')
      }
      setOpen(false)
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to save rule')
    } finally {
      setSaving(false)
    }
  }

  return (
    <PageContainer>
      <div className="mb-4 flex items-center justify-between gap-2">
        <h1 className="text-xl font-semibold">Automation Rules</h1>
        <Button variant="outline" size="sm" onClick={openCreate}>
          <Plus className="mr-1.5 h-3.5 w-3.5" />New rule
        </Button>
      </div>

      {loading ? (
        <SkeletonList count={3}>
          {(i) => <div key={i} className="h-14 rounded-md bg-muted" />}
        </SkeletonList>
      ) : rules.length === 0 ? (
        <EmptyState message="No rules yet. Create one to automate categorisation or archiving." />
      ) : (
        <DividedList>
          {rules.map((rule) => (
            <div key={rule.id} className="flex items-center gap-3 py-3">
              <Switch
                checked={rule.enabled}
                onCheckedChange={() => onToggle(rule)}
                aria-label="Toggle rule"
              />
              <div className="min-w-0 flex-1">
                <p className="truncate font-medium">{rule.name}</p>
                <p className="text-xs text-muted-foreground">
                  <Badge variant="outline" className="mr-1 text-[10px]">
                    {TRIGGER_OPTIONS.find((o) => o.value === rule.trigger)?.label ?? rule.trigger}
                  </Badge>
                  priority {rule.priority} &middot; {rule.conditions?.length ?? 0} condition(s) &middot;{' '}
                  {rule.actions?.length ?? 0} action(s)
                </p>
              </div>
              <Button variant="ghost" size="icon" className="h-7 w-7" onClick={() => openEdit(rule)}>
                <Pencil className="h-3.5 w-3.5" />
              </Button>
              <Button variant="ghost" size="icon" className="h-7 w-7 text-muted-foreground hover:text-destructive" onClick={() => void onDelete(rule.id)}>
                <Trash2 className="h-3.5 w-3.5" />
              </Button>
            </div>
          ))}
        </DividedList>
      )}

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-w-xl">
          <DialogHeader>
            <DialogTitle>{editing ? 'Edit rule' : 'New rule'}</DialogTitle>
          </DialogHeader>

          <div className="space-y-4 py-2">
            <div className="space-y-1.5">
              <Label>Name</Label>
              <Input
                value={form.name}
                onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))}
                placeholder="e.g. Tag GitHub repos"
              />
            </div>

            <div className="grid grid-cols-2 gap-4">
              <div className="space-y-1.5">
                <Label>Trigger</Label>
                <Select
                  value={form.trigger}
                  onValueChange={(v) => setForm((f) => ({ ...f, trigger: v as RuleTrigger }))}
                >
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {TRIGGER_OPTIONS.map((o) => (
                      <SelectItem key={o.value} value={o.value}>
                        {o.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="space-y-1.5">
                <Label>Priority (0 = highest)</Label>
                <Input
                  type="number"
                  min={0}
                  max={9999}
                  value={form.priority}
                  onChange={(e) => setForm((f) => ({ ...f, priority: e.target.value }))}
                />
              </div>
            </div>

            <div className="flex items-center gap-2">
              <Switch
                id="rule-enabled"
                checked={form.enabled}
                onCheckedChange={(v) => setForm((f) => ({ ...f, enabled: v }))}
              />
              <Label htmlFor="rule-enabled">Enabled</Label>
            </div>

            <div className="space-y-1.5">
              <Label>Conditions (JSON array)</Label>
              <JsonEditor
                value={form.conditionsJson}
                onChange={(v) => setForm((f) => ({ ...f, conditionsJson: v }))}
                minHeight="100px"
                maxHeight="200px"
              />
              <p className="text-xs text-muted-foreground">
                Each object: field, op, value. Fields: kind, status, url_contains,
                title_contains, category_name. Ops: eq, ne, contains, not_contains.
              </p>
            </div>

            <div className="space-y-1.5">
              <Label>Actions (JSON array)</Label>
              <JsonEditor
                value={form.actionsJson}
                onChange={(v) => setForm((f) => ({ ...f, actionsJson: v }))}
                minHeight="100px"
                maxHeight="200px"
              />
              <p className="text-xs text-muted-foreground">
                Types: add_category (params.category_name), set_status (params.status), archive.
              </p>
            </div>
          </div>

          <DialogActions>
            <Button variant="outline" onClick={() => setOpen(false)}>
              Cancel
            </Button>
            <Button onClick={onSave} disabled={saving || !form.name.trim()}>
              {saving ? 'Saving...' : 'Save'}
            </Button>
          </DialogActions>
        </DialogContent>
      </Dialog>
    </PageContainer>
  )
}
