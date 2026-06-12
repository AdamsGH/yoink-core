import { ExternalLink, FolderMinus, GitFork, Search, Star } from 'lucide-react'
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
  IconButton,
  Input,
  Label,
  SkeletonList,
} from '@ui'
import { CompactCardHeader, EmptyState, PageContainer } from '@app'

import {
  addStarToFolder,
  deleteFolder,
  listFolders,
  listFolderStars,
  listGhStars,
  removeStarFromFolder,
  updateFolder,
} from '@inbox/api/items'
import type { InboxGhFolder, InboxGhFolderCreate, InboxGhStar } from '@inbox/types'

export default function StarFolderPage() {
  const { id } = useParams<{ id: string }>()
  const folderId = Number(id)
  const navigate = useNavigate()

  const [folder, setFolder] = useState<InboxGhFolder | null>(null)
  const [stars, setStars] = useState<InboxGhStar[]>([])
  const [allStars, setAllStars] = useState<InboxGhStar[]>([])
  const [addSearch, setAddSearch] = useState('')
  const [loading, setLoading] = useState(true)
  const [editing, setEditing] = useState(false)
  const [form, setForm] = useState<InboxGhFolderCreate>({ name: '' })

  useEffect(() => {
    if (!folderId) return
    Promise.all([
      listFolders(),
      listFolderStars(folderId),
      listGhStars({ limit: 200 }),
    ])
      .then(([folders, folderStars, allStarsRes]) => {
        const f = folders.find((x) => x.id === folderId) ?? null
        setFolder(f)
        if (f) setForm({ name: f.name, description: f.description ?? '', icon: f.icon ?? '' })
        setStars(folderStars)
        setAllStars(allStarsRes.items)
      })
      .catch((err) => {
        if (import.meta.env.DEV) console.error(err)
        toast.error('Failed to load folder')
      })
      .finally(() => setLoading(false))
  }, [folderId])

  const starIdsInFolder = new Set(stars.map((s) => s.id))
  const addCandidates = allStars.filter(
    (s) => !starIdsInFolder.has(s.id) &&
      (!addSearch || s.full_name.toLowerCase().includes(addSearch.toLowerCase()) ||
        (s.description ?? '').toLowerCase().includes(addSearch.toLowerCase()))
  )

  async function onSave() {
    if (!folder) return
    try {
      const updated = await updateFolder(folder.id, form)
      setFolder(updated)
      setEditing(false)
      toast.success('Folder updated')
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to update')
    }
  }

  async function onDeleteFolder() {
    if (!folder) return
    try {
      await deleteFolder(folder.id)
      toast.success('Folder deleted')
      navigate('/inbox/stars')
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to delete')
    }
  }

  async function onRemoveStar(starId: number) {
    try {
      await removeStarFromFolder(folderId, starId)
      setStars((p) => p.filter((s) => s.id !== starId))
      toast.success('Removed from folder')
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to remove')
    }
  }

  async function onAddStar(starId: number) {
    try {
      await addStarToFolder(folderId, starId)
      const star = allStars.find((s) => s.id === starId)!
      setStars((p) => [...p, star])
      toast.success('Added to folder')
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to add')
    }
  }

  if (!loading && !folder) {
    return (
      <PageContainer>
        <div className="container mx-auto max-w-5xl py-8">
          <EmptyState message="Folder not found." />
        </div>
      </PageContainer>
    )
  }

  return (
    <PageContainer>
    <div className="container mx-auto max-w-5xl space-y-4 py-4">
      <Card>
        <CompactCardHeader
          title={folder ? `${folder.icon ? folder.icon + ' ' : ''}${folder.name}` : '…'}
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
                    <AlertDialogTitle>Delete folder?</AlertDialogTitle>
                    <AlertDialogDescription>Stars are not deleted, only unlinked from this folder.</AlertDialogDescription>
                  </AlertDialogHeader>
                  <AlertDialogFooter>
                    <AlertDialogCancel>Cancel</AlertDialogCancel>
                    <AlertDialogAction onClick={onDeleteFolder}>Delete</AlertDialogAction>
                  </AlertDialogFooter>
                </AlertDialogContent>
              </AlertDialog>
            </div>
          }
        />
        {editing && folder && (
          <CardContent className="space-y-3 pt-0">
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1">
                <Label>Name</Label>
                <Input value={form.name} onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))} />
              </div>
              <div className="space-y-1">
                <Label>Icon</Label>
                <Input value={form.icon ?? ''} placeholder="e.g. 🔧" onChange={(e) => setForm((f) => ({ ...f, icon: e.target.value }))} />
              </div>
            </div>
            <div className="space-y-1">
              <Label>Description</Label>
              <Input value={form.description ?? ''} onChange={(e) => setForm((f) => ({ ...f, description: e.target.value }))} />
            </div>
            <Button onClick={onSave} size="sm">Save</Button>
          </CardContent>
        )}
      </Card>

      {loading ? (
        <SkeletonList count={6}>{(i) => <Card key={i}><CardContent className="h-28" /></Card>}</SkeletonList>
      ) : stars.length === 0 ? (
        <EmptyState message="No stars in this folder yet. Add some below." />
      ) : (
        <div className="grid grid-cols-1 gap-3 md:grid-cols-2 lg:grid-cols-3">
          {stars.map((s) => (
            <Card key={s.id} className="flex flex-col">
              <CardContent className="flex flex-1 flex-col gap-2 p-4">
                <div className="flex items-start gap-2">
                  {s.owner_avatar_url && (
                    <img src={s.owner_avatar_url} alt="" className="h-6 w-6 flex-shrink-0 rounded" />
                  )}
                  <a href={s.html_url} target="_blank" rel="noopener noreferrer"
                    className="line-clamp-1 flex-1 font-medium hover:underline" title={s.full_name}>
                    {s.full_name}
                  </a>
                  <div className="flex flex-shrink-0 gap-1">
                    <IconButton tooltip="Open" onClick={() => window.open(s.html_url, '_blank')}><ExternalLink className="h-3.5 w-3.5" /></IconButton>
                    <IconButton tooltip="Remove from folder" onClick={() => onRemoveStar(s.id)}><FolderMinus className="h-3.5 w-3.5" /></IconButton>
                  </div>
                </div>
                {s.description && (
                  <p className="line-clamp-2 flex-1 text-sm text-muted-foreground">{s.description}</p>
                )}
                <div className="flex flex-wrap items-center gap-1.5 text-xs">
                  {s.language && <Badge variant="outline">{s.language}</Badge>}
                  <span className="flex items-center gap-0.5 text-muted-foreground">
                    <Star className="h-3 w-3" />{s.stargazers_count.toLocaleString()}
                  </span>
                  {s.fork && <span className="text-muted-foreground"><GitFork className="h-3 w-3" /></span>}
                  {s.archived && <Badge variant="destructive">archived</Badge>}
                </div>
                {(s.topics?.length ?? 0) > 0 && (
                  <div className="flex flex-wrap gap-1">
                    {s.topics!.slice(0, 4).map((t) => (
                      <Badge key={t} variant="secondary" className="text-[10px] font-normal">{t}</Badge>
                    ))}
                  </div>
                )}
              </CardContent>
            </Card>
          ))}
        </div>
      )}

      {/* Add stars section */}
      <div className="space-y-3 rounded-lg border p-4">
        <h3 className="text-sm font-semibold">Add stars to this folder</h3>
        <div className="relative">
          <Search className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
          <Input
            className="pl-8"
            placeholder="Search your stars..."
            value={addSearch}
            onChange={(e) => setAddSearch(e.target.value)}
          />
        </div>
        {addCandidates.length === 0 ? (
          <p className="text-sm text-muted-foreground">All stars are already in this folder.</p>
        ) : (
          <div className="max-h-64 space-y-1 overflow-y-auto">
            {addCandidates.slice(0, 30).map((s) => (
              <div key={s.id} className="flex items-center justify-between rounded px-2 py-1.5 hover:bg-muted">
                <div className="min-w-0 flex-1">
                  <span className="truncate text-sm font-medium">{s.full_name}</span>
                  {s.language && (
                    <span className="ml-2 text-xs text-muted-foreground">{s.language}</span>
                  )}
                </div>
                <Button size="sm" variant="outline" onClick={() => onAddStar(s.id)}>Add</Button>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
    </PageContainer>
  )
}
