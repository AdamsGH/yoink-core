import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Check, ChevronDown, Plus, Trash2, X } from 'lucide-react'

import { botSettingsApi } from '@core/lib/api'
import { Badge, Button, Card, CardContent, CardDescription, CardHeader, CardTitle, Input, Popover, PopoverContent, PopoverTrigger, Separator, Skeleton } from '@ui'
import { toast } from '@core/components/ui/toast'
import type { AvailableFeature, TagMapEntry } from '@core/types/api'

function FeatureMultiSelect({
  value,
  onChange,
  available,
}: {
  value: string[]
  onChange: (v: string[]) => void
  available: AvailableFeature[]
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const [search, setSearch] = useState('')

  const filtered = available.filter(
    (f) =>
      f.label.toLowerCase().includes(search.toLowerCase()) ||
      f.key.toLowerCase().includes(search.toLowerCase()),
  )

  const toggle = (key: string) => {
    onChange(value.includes(key) ? value.filter((k) => k !== key) : [...value, key])
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          className="flex min-h-9 w-full flex-wrap items-center gap-1 rounded-md border border-input bg-background px-3 py-1.5 text-sm ring-offset-background focus:outline-none focus:ring-2 focus:ring-ring"
        >
          {value.length === 0 ? (
            <span className="text-muted-foreground">{t('tag_map.select_features')}</span>
          ) : (
            value.map((k) => {
              const f = available.find((a) => a.key === k)
              return (
                <Badge key={k} variant="secondary" className="gap-1 pr-1">
                  {f?.label ?? k}
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation()
                      toggle(k)
                    }}
                    className="rounded-sm opacity-70 hover:opacity-100"
                  >
                    <X className="h-3 w-3" />
                  </button>
                </Badge>
              )
            })
          )}
          <ChevronDown className="ml-auto h-4 w-4 shrink-0 opacity-50" />
        </button>
      </PopoverTrigger>
      <PopoverContent className="w-72 p-0" align="start">
        <div className="p-2">
          <Input
            placeholder={t('tag_map.search_features')}
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="h-8"
          />
        </div>
        <Separator />
        <div className="max-h-52 overflow-y-auto p-1">
          {filtered.length === 0 ? (
            <p className="py-4 text-center text-sm text-muted-foreground">
              {t('common.no_results')}
            </p>
          ) : (
            filtered.map((f) => {
              const selected = value.includes(f.key)
              return (
                <button
                  key={f.key}
                  type="button"
                  onClick={() => toggle(f.key)}
                  className="flex w-full items-start gap-2 rounded-sm px-2 py-1.5 text-sm hover:bg-accent"
                >
                  <Check
                    className={`mt-0.5 h-4 w-4 shrink-0 ${selected ? 'opacity-100' : 'opacity-0'}`}
                  />
                  <div className="text-left">
                    <div className="font-medium">{f.label}</div>
                    <div className="text-xs text-muted-foreground">{f.key}</div>
                  </div>
                </button>
              )
            })
          )}
        </div>
      </PopoverContent>
    </Popover>
  )
}

interface TagRowProps {
  entry: TagMapEntry
  available: AvailableFeature[]
  onChange: (entry: TagMapEntry) => void
  onDelete: () => void
}

function TagRow({ entry, available, onChange, onDelete }: TagRowProps) {
  const { t } = useTranslation()

  return (
    <div className="flex items-start gap-2">
      <div className="w-36 shrink-0">
        <Input
          placeholder={t('tag_map.tag_placeholder')}
          value={entry.tag}
          onChange={(e) => onChange({ ...entry, tag: e.target.value })}
          className="h-9"
        />
      </div>
      <div className="flex-1 min-w-0">
        <FeatureMultiSelect
          value={entry.features}
          onChange={(features) => onChange({ ...entry, features })}
          available={available}
        />
      </div>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="h-9 w-9 shrink-0 text-muted-foreground hover:text-destructive"
        onClick={onDelete}
        title={t('common.delete')}
      >
        <Trash2 className="h-4 w-4" />
      </Button>
    </div>
  )
}

export default function TagMapEditor({ embedded = false }: { embedded?: boolean }) {
  const { t } = useTranslation()
  const [entries, setEntries] = useState<TagMapEntry[]>([])
  const [available, setAvailable] = useState<AvailableFeature[]>([])
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const initialized = useRef(false)

  useEffect(() => {
    Promise.all([
      botSettingsApi.getTagMap(),
      botSettingsApi.getAvailableFeatures(),
    ])
      .then(([mapRes, featRes]) => {
        setEntries(mapRes.data)
        setAvailable(featRes.data)
      })
      .catch(() => toast.error(t('tag_map.load_error')))
      .finally(() => {
        setLoading(false)
        initialized.current = true
      })
  }, [t])

  const addRow = () => setEntries((prev) => [...prev, { tag: '', features: [] }])

  const updateRow = (i: number, entry: TagMapEntry) =>
    setEntries((prev) => prev.map((e, idx) => (idx === i ? entry : e)))

  const deleteRow = (i: number) =>
    setEntries((prev) => prev.filter((_, idx) => idx !== i))

  const save = async () => {
    const valid = entries.filter((e) => e.tag.trim())
    setSaving(true)
    try {
      const res = await botSettingsApi.putTagMap(valid)
      setEntries(res.data)
      toast.success(t('tag_map.save_ok'))
    } catch {
      toast.error(t('tag_map.save_error'))
    } finally {
      setSaving(false)
    }
  }

  const content = (
    <div className="space-y-4">
      {loading ? (
        <div className="space-y-2">
          <Skeleton className="h-9 w-full" />
          <Skeleton className="h-9 w-full" />
        </div>
      ) : entries.length === 0 ? (
        <p className="text-sm text-muted-foreground">{t('tag_map.empty')}</p>
      ) : (
        <div className="space-y-2">
          <div className="grid grid-cols-[144px_1fr_36px] gap-2 px-0.5 text-xs font-medium text-muted-foreground">
            <span>{t('tag_map.col_tag')}</span>
            <span>{t('tag_map.col_features')}</span>
            <span />
          </div>
          {entries.map((entry, i) => (
            <TagRow
              key={i}
              entry={entry}
              available={available}
              onChange={(e) => updateRow(i, e)}
              onDelete={() => deleteRow(i)}
            />
          ))}
        </div>
      )}
      <div className="grid grid-cols-2 gap-2 pt-1">
        <Button type="button" variant="outline" size="sm" onClick={addRow} className="w-full">
          <Plus className="mr-1.5 h-4 w-4" />
          {t('tag_map.add_rule')}
        </Button>
        <Button type="button" size="sm" onClick={save} disabled={saving || loading} className="w-full">
          {saving ? t('tag_map.saving') : t('tag_map.save')}
        </Button>
      </div>
    </div>
  )

  if (embedded) return content

  return (
    <Card>
      <CardHeader className="px-4 py-3">
        <CardTitle>{t('tag_map.title')}</CardTitle>
        <CardDescription>{t('tag_map.description')}</CardDescription>
      </CardHeader>
      <CardContent className="px-4 pb-4">{content}</CardContent>
    </Card>
  )
}
