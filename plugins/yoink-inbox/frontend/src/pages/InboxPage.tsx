import { ExternalLink, RefreshCw, Trash2 } from 'lucide-react'
import { useEffect, useState } from 'react'

import {
  Badge,
  Button,
  Card,
  CardContent,
  Input,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  SkeletonList,
} from '@ui'
import { EmptyState, PageContainer } from '@app'
import { toast } from 'sonner'

import {
  deleteItem,
  listCategories,
  listItems,
  reclassifyItem,
} from '@inbox/api/items'
import type { InboxCategory, InboxItem } from '@inbox/types'

const PAGE_SIZE = 25

const STATUS_OPTIONS = [
  { value: 'all', label: 'All statuses' },
  { value: 'pending', label: 'Pending' },
  { value: 'enriched', label: 'Enriched' },
  { value: 'classified', label: 'Classified' },
  { value: 'archived', label: 'Archived' },
  { value: 'failed', label: 'Failed' },
]

const KIND_OPTIONS = [
  { value: 'all', label: 'All kinds' },
  { value: 'link', label: 'Link' },
  { value: 'github_repo', label: 'GitHub' },
  { value: 'article', label: 'Article' },
  { value: 'video', label: 'Video' },
  { value: 'other', label: 'Other' },
]

function relTime(iso: string): string {
  const diff = Math.floor((Date.now() - new Date(iso).getTime()) / 1000)
  if (diff < 60) return `${diff}s ago`
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`
  return `${Math.floor(diff / 86400)}d ago`
}

export default function InboxPage() {
  const [items, setItems] = useState<InboxItem[]>([])
  const [categories, setCategories] = useState<InboxCategory[]>([])
  const [loading, setLoading] = useState(true)
  const [moreCursor, setMoreCursor] = useState<string | null>(null)
  const [statusFilter, setStatusFilter] = useState('all')
  const [kindFilter, setKindFilter] = useState('all')
  const [search, setSearch] = useState('')
  const [debouncedSearch, setDebouncedSearch] = useState('')

  useEffect(() => {
    const t = setTimeout(() => setDebouncedSearch(search), 350)
    return () => clearTimeout(t)
  }, [search])

  useEffect(() => {
    setLoading(true)
    Promise.all([
      listItems({
        limit: PAGE_SIZE,
        status: statusFilter === 'all' ? undefined : statusFilter,
        kind: kindFilter === 'all' ? undefined : kindFilter,
        search: debouncedSearch.length >= 2 ? debouncedSearch : undefined,
      }),
      listCategories(),
    ])
      .then(([list, cats]) => {
        setItems(list.items)
        setMoreCursor(list.next_cursor)
        setCategories(cats)
      })
      .catch((err) => {
        if (import.meta.env.DEV) console.error(err)
        toast.error('Failed to load inbox')
      })
      .finally(() => setLoading(false))
  }, [statusFilter, kindFilter, debouncedSearch])

  async function loadMore() {
    if (!moreCursor) return
    try {
      const list = await listItems({
        limit: PAGE_SIZE,
        cursor: moreCursor,
        status: statusFilter === 'all' ? undefined : statusFilter,
        kind: kindFilter === 'all' ? undefined : kindFilter,
        search: debouncedSearch.length >= 2 ? debouncedSearch : undefined,
      })
      setItems((prev) => [...prev, ...list.items])
      setMoreCursor(list.next_cursor)
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to load more')
    }
  }

  async function onDelete(id: number) {
    try {
      await deleteItem(id)
      setItems((prev) => prev.filter((i) => i.id !== id))
      toast.success('Archived')
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to archive')
    }
  }

  async function onReclassify(id: number) {
    try {
      const r = await reclassifyItem(id)
      toast.success(`Reclassify ${r.status}`)
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to enqueue reclassify')
    }
  }

  return (
    <PageContainer>
      {/* Toolbar */}
      <div className="mb-4 flex flex-col gap-2">
        <div className="flex items-center justify-between">
          <h1 className="text-xl font-semibold">Inbox</h1>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Input
            placeholder="Search title, url, summary..."
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="h-8 min-w-0 flex-1 basis-48"
          />
          <Select value={statusFilter} onValueChange={setStatusFilter}>
            <SelectTrigger className="h-8 w-36">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {STATUS_OPTIONS.map((o) => (
                <SelectItem key={o.value} value={o.value}>{o.label}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Select value={kindFilter} onValueChange={setKindFilter}>
            <SelectTrigger className="h-8 w-32">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {KIND_OPTIONS.map((o) => (
                <SelectItem key={o.value} value={o.value}>{o.label}</SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        {categories.length > 0 && (
          <div className="flex flex-wrap gap-1.5">
            {categories.map((c) => (
              <Badge key={c.id} variant="secondary" className="h-6 font-normal">
                {c.name}
                <span className="ml-1 text-muted-foreground/70">{c.item_count}</span>
              </Badge>
            ))}
          </div>
        )}
      </div>

      {/* List */}
      {loading ? (
        <SkeletonList count={5}>
          {(i) => <Card key={i}><CardContent className="h-20" /></Card>}
        </SkeletonList>
      ) : items.length === 0 ? (
        <EmptyState message="No items match the current filters." />
      ) : (
        <div className="space-y-2">
          {items.map((item) => (
            <Card key={item.id}>
              <CardContent className="flex gap-3 p-3">
                {item.og_image_url && (
                  <img
                    src={item.og_image_url}
                    alt=""
                    className="h-14 w-14 shrink-0 rounded object-cover"
                    onError={(e) => { (e.target as HTMLImageElement).style.display = 'none' }}
                  />
                )}
                <div className="min-w-0 flex-1">
                  <div className="flex items-start justify-between gap-2">
                    <a
                      href={item.url}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="line-clamp-2 text-sm font-medium hover:underline"
                    >
                      {item.title ?? item.url}
                    </a>
                    <div className="flex shrink-0 items-center gap-0.5">
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-7 w-7"
                        title="Open"
                        onClick={() => window.open(item.url, '_blank')}
                      >
                        <ExternalLink className="h-3.5 w-3.5" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-7 w-7"
                        title="Reclassify"
                        onClick={() => void onReclassify(item.id)}
                      >
                        <RefreshCw className="h-3.5 w-3.5" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-7 w-7 text-muted-foreground hover:text-destructive"
                        title="Archive"
                        onClick={() => void onDelete(item.id)}
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </Button>
                    </div>
                  </div>
                  {item.summary && (
                    <p className="mt-0.5 line-clamp-2 text-xs text-muted-foreground">
                      {item.summary}
                    </p>
                  )}
                  <div className="mt-1.5 flex flex-wrap items-center gap-1">
                    <Badge variant="outline" className="h-5 px-1.5 text-[10px]">{item.kind}</Badge>
                    <Badge
                      variant={item.status === 'failed' ? 'destructive' : 'secondary'}
                      className="h-5 px-1.5 text-[10px]"
                    >
                      {item.status}
                    </Badge>
                    {item.llm_status && item.llm_status !== 'success' && (
                      <Badge
                        variant={item.llm_status === 'failed' ? 'destructive' : 'outline'}
                        className="h-5 px-1.5 text-[10px]"
                      >
                        llm:{item.llm_status}
                      </Badge>
                    )}
                    {item.categories.map((c) => (
                      <Badge key={c.id} variant="default" className="h-5 px-1.5 text-[10px]">
                        {c.name}
                      </Badge>
                    ))}
                    <span className="ml-auto text-[10px] text-muted-foreground">
                      {relTime(item.created_at)}
                    </span>
                  </div>
                </div>
              </CardContent>
            </Card>
          ))}
          {moreCursor && (
            <div className="flex justify-center pt-2">
              <Button variant="outline" size="sm" onClick={() => void loadMore()}>
                Load more
              </Button>
            </div>
          )}
        </div>
      )}
    </PageContainer>
  )
}
