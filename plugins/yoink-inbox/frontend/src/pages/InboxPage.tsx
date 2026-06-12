import { ExternalLink, RefreshCw, Trash2 } from 'lucide-react'
import { useEffect, useState } from 'react'

import { Badge, Button, Card, CardContent, Input, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, SkeletonList } from '@ui'
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
  { value: 'all', label: 'All' },
  { value: 'pending', label: 'Pending' },
  { value: 'enriched', label: 'Enriched' },
  { value: 'classified', label: 'Classified' },
  { value: 'archived', label: 'Archived' },
  { value: 'failed', label: 'Failed' },
]

const KIND_OPTIONS = [
  { value: 'all', label: 'All kinds' },
  { value: 'link', label: 'Link' },
  { value: 'github_repo', label: 'GitHub repo' },
  { value: 'article', label: 'Article' },
  { value: 'video', label: 'Video' },
  { value: 'other', label: 'Other' },
]

function relTime(iso: string): string {
  const now = Date.now()
  const then = new Date(iso).getTime()
  const diff = Math.floor((now - then) / 1000)
  if (diff < 60) return `${diff}s ago`
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`
  return `${Math.floor(diff / 86400)}d ago`
}

export default function InboxPage() {
  const [items, setItems] = useState<InboxItem[]>([])
  const [categories, setCategories] = useState<InboxCategory[]>([])
  const [loading, setLoading] = useState(true)
  const [cursor, setCursor] = useState<string | null>(null)
  const [moreCursor, setMoreCursor] = useState<string | null>(null)
  const [statusFilter, setStatusFilter] = useState('all')
  const [kindFilter, setKindFilter] = useState('all')
  const [search, setSearch] = useState('')

  // Cheap debounce: hold the field value, push to the actual query after 350ms
  // of idle. Avoids one network call per keystroke without bringing in lodash.
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
        setCursor(null)
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
      setCursor(moreCursor)
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
    <div className="container mx-auto max-w-5xl space-y-4 py-4">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <h1 className="text-xl font-semibold">Inbox</h1>
        <div className="flex flex-wrap gap-2">
          <Input
            placeholder="Search title, url, summary..."
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="w-full sm:w-64"
          />
          <Select value={statusFilter} onValueChange={setStatusFilter}>
            <SelectTrigger className="w-36">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {STATUS_OPTIONS.map((o) => (
                <SelectItem key={o.value} value={o.value}>{o.label}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Select value={kindFilter} onValueChange={setKindFilter}>
            <SelectTrigger className="w-36">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {KIND_OPTIONS.map((o) => (
                <SelectItem key={o.value} value={o.value}>{o.label}</SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </div>

      {categories.length > 0 && (
        <div className="flex flex-wrap gap-1.5">
          {categories.map((c) => (
            <Badge key={c.id} variant="secondary" className="font-normal">
              {c.name}
              <span className="ml-1 text-muted-foreground">{c.item_count}</span>
            </Badge>
          ))}
        </div>
      )}

      {loading && cursor === null ? (
        <SkeletonList count={5}>
          {(i) => (
            <Card key={i}>
              <CardContent className="h-24" />
            </Card>
          )}
        </SkeletonList>
      ) : items.length === 0 ? (
        <EmptyState message="No items match the current filters." />
      ) : (
        <div className="space-y-2">
          {items.map((item) => (
            <Card key={item.id}>
              <CardContent className="flex gap-3 p-3">
                {item.og_image_url ? (
                  // eslint-disable-next-line @next/next/no-img-element
                  <img
                    src={item.og_image_url}
                    alt=""
                    className="h-16 w-16 flex-shrink-0 rounded object-cover"
                    onError={(e) => {
                      ;(e.target as HTMLImageElement).style.display = 'none'
                    }}
                  />
                ) : null}
                <div className="min-w-0 flex-1">
                  <div className="flex items-start justify-between gap-2">
                    <a
                      href={item.url}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="line-clamp-2 font-medium hover:underline"
                    >
                      {item.title || item.url}
                    </a>
                    <div className="flex flex-shrink-0 gap-1">
                      <Button
                        variant="ghost"
                        size="icon"
                        title="External"
                        onClick={() => window.open(item.url, '_blank')}
                      >
                        <ExternalLink className="h-4 w-4" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        title="Reclassify"
                        onClick={() => onReclassify(item.id)}
                      >
                        <RefreshCw className="h-4 w-4" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        title="Archive"
                        onClick={() => onDelete(item.id)}
                      >
                        <Trash2 className="h-4 w-4" />
                      </Button>
                    </div>
                  </div>
                  {item.summary && (
                    <p className="mt-1 line-clamp-2 text-sm text-muted-foreground">
                      {item.summary}
                    </p>
                  )}
                  <div className="mt-2 flex flex-wrap items-center gap-1.5 text-xs">
                    <Badge variant="outline">{item.kind}</Badge>
                    <Badge
                      variant={item.status === 'failed' ? 'destructive' : 'secondary'}
                    >
                      {item.status}
                    </Badge>
                    {item.llm_status && (
                      <Badge
                        variant={item.llm_status === 'failed' ? 'destructive' : 'outline'}
                      >
                        llm: {item.llm_status}
                      </Badge>
                    )}
                    {item.categories.map((c) => (
                      <Badge key={c.id} variant="default">
                        {c.name}
                      </Badge>
                    ))}
                    <span className="ml-auto text-muted-foreground">
                      {relTime(item.created_at)}
                    </span>
                  </div>
                </div>
              </CardContent>
            </Card>
          ))}
          {moreCursor && (
            <div className="flex justify-center pt-2">
              <Button variant="outline" onClick={loadMore}>
                Load more
              </Button>
            </div>
          )}
        </div>
      )}
    </div>
    </PageContainer>
  )
}
