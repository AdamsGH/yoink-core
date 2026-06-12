import { ExternalLink, RefreshCw, Trash2 } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useNavigate, useParams } from 'react-router'
import { toast } from 'sonner'

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
  Badge,
  Button,
  Card,
  CardContent,
  Input,
  Label,
  IconButton,
  SkeletonList,
  Textarea,
} from '@ui'
import { CompactCardHeader, EmptyState } from '@app'

import {
  deleteCategory,
  listCategories,
  listItems,
  reclassifyItem,
  deleteItem,
  updateCategory,
} from '@inbox/api/items'
import type { InboxCategory, InboxCategoryCreate, InboxItem } from '@inbox/types'

const PAGE_SIZE = 25

function relTime(iso: string): string {
  const diff = Math.floor((Date.now() - new Date(iso).getTime()) / 1000)
  if (diff < 60) return `${diff}s ago`
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`
  return `${Math.floor(diff / 86400)}d ago`
}

export default function CategoryPage() {
  const { slug } = useParams<{ slug: string }>()
  const navigate = useNavigate()

  const [category, setCategory] = useState<InboxCategory | null>(null)
  const [items, setItems] = useState<InboxItem[]>([])
  const [moreCursor, setMoreCursor] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [editing, setEditing] = useState(false)
  const [form, setForm] = useState<InboxCategoryCreate>({ name: '' })

  useEffect(() => {
    if (!slug) return
    listCategories()
      .then((cats) => {
        const cat = cats.find((c) => c.slug === slug) ?? null
        setCategory(cat)
        if (cat) setForm({ name: cat.name, icon: cat.icon ?? '', color: cat.color ?? '', description: cat.description ?? '' })
        return cat
      })
      .catch((err) => {
        if (import.meta.env.DEV) console.error(err)
        toast.error('Failed to load category')
      })
  }, [slug])

  useEffect(() => {
    if (!category) return
    setLoading(true)
    listItems({ limit: PAGE_SIZE, category_id: category.id })
      .then((res) => {
        setItems(res.items)
        setMoreCursor(res.next_cursor)
      })
      .catch((err) => {
        if (import.meta.env.DEV) console.error(err)
      })
      .finally(() => setLoading(false))
  }, [category])

  async function loadMore() {
    if (!category || !moreCursor) return
    const res = await listItems({ limit: PAGE_SIZE, category_id: category.id, cursor: moreCursor })
    setItems((p) => [...p, ...res.items])
    setMoreCursor(res.next_cursor)
  }

  async function onSave() {
    if (!category) return
    try {
      const updated = await updateCategory(category.id, form)
      setCategory(updated)
      setEditing(false)
      toast.success('Category updated')
      navigate(`/inbox/categories/${updated.slug}`, { replace: true })
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to update')
    }
  }

  async function onDelete() {
    if (!category) return
    try {
      await deleteCategory(category.id)
      toast.success('Category deleted')
      navigate('/inbox')
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to delete')
    }
  }

  async function onArchiveItem(id: number) {
    try {
      await deleteItem(id)
      setItems((p) => p.filter((i) => i.id !== id))
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to archive')
    }
  }

  if (!loading && !category) {
    return (
      <div className="container mx-auto max-w-4xl py-8">
        <EmptyState message="Category not found." />
      </div>
    )
  }

  return (
    <div className="container mx-auto max-w-4xl space-y-4 py-4">
      <Card>
        <CompactCardHeader
          title={category?.name ?? '…'}
          actions={
            <div className="flex gap-2">
              <Button variant="outline" size="sm" onClick={() => setEditing((e) => !e)}>
                {editing ? 'Cancel' : 'Edit'}
              </Button>
              <AlertDialog>
                <AlertDialogTrigger asChild>
                  <Button variant="destructive" size="sm">Delete</Button>
                </AlertDialogTrigger>
                <AlertDialogContent>
                  <AlertDialogHeader>
                    <AlertDialogTitle>Delete category?</AlertDialogTitle>
                    <AlertDialogDescription>
                      This removes the category and all its item bindings. Items themselves are not deleted.
                    </AlertDialogDescription>
                  </AlertDialogHeader>
                  <AlertDialogFooter>
                    <AlertDialogCancel>Cancel</AlertDialogCancel>
                    <AlertDialogAction onClick={onDelete}>Delete</AlertDialogAction>
                  </AlertDialogFooter>
                </AlertDialogContent>
              </AlertDialog>
            </div>
          }
        />
        {editing && category && (
          <CardContent className="space-y-3 pt-0">
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1">
                <Label>Name</Label>
                <Input value={form.name} onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))} />
              </div>
              <div className="space-y-1">
                <Label>Icon</Label>
                <Input value={form.icon ?? ''} placeholder="e.g. 📚" onChange={(e) => setForm((f) => ({ ...f, icon: e.target.value }))} />
              </div>
              <div className="space-y-1">
                <Label>Color</Label>
                <Input type="color" value={form.color ?? '#6366f1'} onChange={(e) => setForm((f) => ({ ...f, color: e.target.value }))} />
              </div>
            </div>
            <div className="space-y-1">
              <Label>Description</Label>
              <Textarea value={form.description ?? ''} onChange={(e) => setForm((f) => ({ ...f, description: e.target.value }))} rows={2} />
            </div>
            <Button onClick={onSave} size="sm">Save</Button>
          </CardContent>
        )}
      </Card>

      {loading ? (
        <SkeletonList count={4}>{(i) => <Card key={i}><CardContent className="h-20" /></Card>}</SkeletonList>
      ) : items.length === 0 ? (
        <EmptyState message="No items in this category yet." />
      ) : (
        <div className="space-y-2">
          {items.map((item) => (
            <Card key={item.id}>
              <CardContent className="flex gap-3 p-3">
                {item.og_image_url && (
                  <img src={item.og_image_url} alt="" className="h-14 w-14 flex-shrink-0 rounded object-cover"
                    onError={(e) => { (e.target as HTMLImageElement).style.display = 'none' }} />
                )}
                <div className="min-w-0 flex-1">
                  <div className="flex items-start justify-between gap-2">
                    <a href={item.url} target="_blank" rel="noopener noreferrer"
                      className="line-clamp-2 font-medium hover:underline">
                      {item.title || item.url}
                    </a>
                    <div className="flex flex-shrink-0 gap-1">
                      <IconButton tooltip="Open" onClick={() => window.open(item.url, '_blank')}><ExternalLink className="h-4 w-4" /></IconButton>
                      <IconButton tooltip="Reclassify" onClick={() => reclassifyItem(item.id).then(() => toast.success('Queued'))}><RefreshCw className="h-4 w-4" /></IconButton>
                      <IconButton tooltip="Archive" onClick={() => onArchiveItem(item.id)}><Trash2 className="h-4 w-4" /></IconButton>
                    </div>
                  </div>
                  {item.summary && <p className="mt-1 line-clamp-2 text-sm text-muted-foreground">{item.summary}</p>}
                  <div className="mt-1.5 flex flex-wrap items-center gap-1 text-xs">
                    <Badge variant="outline">{item.kind}</Badge>
                    <Badge variant="secondary">{item.status}</Badge>
                    <span className="ml-auto text-muted-foreground">{relTime(item.created_at)}</span>
                  </div>
                </div>
              </CardContent>
            </Card>
          ))}
          {moreCursor && (
            <div className="flex justify-center pt-2">
              <Button variant="outline" onClick={loadMore}>Load more</Button>
            </div>
          )}
        </div>
      )}
    </div>
  )
}
