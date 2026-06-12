import { ExternalLink, FolderOpen, GitFork, RefreshCw, Star } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router'
import { toast } from 'sonner'

import {
  Badge, Button, Card, CardContent, Input,
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
  SkeletonList,
} from '@ui'
import { EmptyState } from '@app'

import { listFolders, listGhStars, triggerGhSync, unstarRepo } from '@inbox/api/items'
import type { InboxGhFolder, InboxGhStar } from '@inbox/types'

const PAGE_SIZE = 30

export default function StarsPage() {
  const navigate = useNavigate()
  const [stars, setStars] = useState<InboxGhStar[]>([])
  const [folders, setFolders] = useState<InboxGhFolder[]>([])
  const [loading, setLoading] = useState(true)
  const [syncStatus, setSyncStatus] = useState<string | null>(null)
  const [lastSync, setLastSync] = useState<string | null>(null)
  const [moreCursor, setMoreCursor] = useState<string | null>(null)
  const [language, setLanguage] = useState('')
  const [search, setSearch] = useState('')
  const [debouncedSearch, setDebouncedSearch] = useState('')
  const [folderId, setFolderId] = useState<string>('all')

  useEffect(() => {
    const t = setTimeout(() => setDebouncedSearch(search), 350)
    return () => clearTimeout(t)
  }, [search])

  useEffect(() => {
    listFolders().then(setFolders).catch(() => {})
  }, [])

  useEffect(() => {
    setLoading(true)
    listGhStars({
      limit: PAGE_SIZE,
      language: language || undefined,
      search: debouncedSearch.length >= 2 ? debouncedSearch : undefined,
      folder_id: folderId !== 'all' ? Number(folderId) : undefined,
    })
      .then((res) => {
        setStars(res.items)
        setMoreCursor(res.next_cursor)
        setSyncStatus(res.sync_status)
        setLastSync(res.last_synced_at)
      })
      .catch((err) => {
        if (import.meta.env.DEV) console.error(err)
        toast.error('Failed to load stars')
      })
      .finally(() => setLoading(false))
  }, [language, debouncedSearch, folderId])

  async function loadMore() {
    if (!moreCursor) return
    try {
      const res = await listGhStars({
        limit: PAGE_SIZE,
        cursor: moreCursor,
        language: language || undefined,
        search: debouncedSearch.length >= 2 ? debouncedSearch : undefined,
      })
      setStars((prev) => [...prev, ...res.items])
      setMoreCursor(res.next_cursor)
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to load more')
    }
  }

  async function onSync() {
    try {
      const r = await triggerGhSync()
      toast.success(`Sync ${r.status}`)
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to enqueue sync')
    }
  }

  return (
    <div className="container mx-auto max-w-6xl space-y-4 py-4">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <h1 className="text-xl font-semibold">GitHub Stars</h1>
          {syncStatus && (
            <p className="text-xs text-muted-foreground">
              Last sync: {syncStatus}
              {lastSync ? ` (${new Date(lastSync).toLocaleString()})` : ''}
            </p>
          )}
        </div>
        <div className="flex flex-wrap gap-2">
          <Input
            placeholder="Search name or description..."
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="w-full sm:w-64"
          />
          <Input
            placeholder="Language"
            value={language}
            onChange={(e) => setLanguage(e.target.value)}
            className="w-32"
          />
          {folders.length > 0 && (
            <Select value={folderId} onValueChange={setFolderId}>
              <SelectTrigger className="w-40">
                <SelectValue placeholder="All folders" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">All folders</SelectItem>
                {folders.map((f) => (
                  <SelectItem key={f.id} value={String(f.id)}>
                    {f.icon ? `${f.icon} ` : ''}{f.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
          <Button variant="outline" onClick={onSync}>
            <RefreshCw className="mr-2 h-4 w-4" />
            Sync
          </Button>
        </div>
      </div>

      {loading ? (
        <SkeletonList count={6}>
          {(i) => (
            <Card key={i}>
              <CardContent className="h-32" />
            </Card>
          )}
        </SkeletonList>
      ) : stars.length === 0 ? (
        <EmptyState message="No starred repos yet. Connect GitHub in settings, then press Sync." />
      ) : (
        <div className="grid grid-cols-1 gap-3 md:grid-cols-2 lg:grid-cols-3">
          {stars.map((s) => (
            <Card key={s.id} className="flex flex-col">
              <CardContent className="flex flex-1 flex-col gap-2 p-4">
                <div className="flex items-start gap-2">
                  {s.owner_avatar_url ? (
                    <img
                      src={s.owner_avatar_url}
                      alt=""
                      className="h-6 w-6 flex-shrink-0 rounded"
                    />
                  ) : null}
                  <a
                    href={s.html_url}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="line-clamp-1 flex-1 font-medium hover:underline"
                    title={s.full_name}
                  >
                    {s.full_name}
                  </a>
                  <Button
                    variant="ghost"
                    size="icon"
                    onClick={() => window.open(s.html_url, '_blank')}
                  >
                    <ExternalLink className="h-3.5 w-3.5" />
                  </Button>
                  {folderId !== 'all' && (
                    <Button
                      variant="ghost"
                      size="icon"
                      title="Open folder"
                      onClick={() => navigate(`/inbox/folders/${folderId}`)}
                    >
                      <FolderOpen className="h-3.5 w-3.5" />
                    </Button>
                  )}
                  {s.can_unstar && (
                    <Button
                      variant="ghost"
                      size="icon"
                      title="Unstar on GitHub"
                      onClick={async () => {
                        try {
                          await unstarRepo(s.id)
                          setStars((p) => p.filter((x) => x.id !== s.id))
                          toast.success('Unstarred')
                        } catch (err) {
                          if (import.meta.env.DEV) console.error(err)
                          toast.error('Failed to unstar')
                        }
                      }}
                    >
                      <Star className="h-3.5 w-3.5 fill-yellow-400 text-yellow-400" />
                    </Button>
                  )}
                </div>
                {s.description && (
                  <p className="line-clamp-3 flex-1 text-sm text-muted-foreground">
                    {s.description}
                  </p>
                )}
                <div className="flex flex-wrap items-center gap-1.5 text-xs">
                  {s.language && <Badge variant="outline">{s.language}</Badge>}
                  <span className="flex items-center gap-0.5 text-muted-foreground">
                    <Star className="h-3 w-3" />
                    {s.stargazers_count.toLocaleString()}
                  </span>
                  {s.fork && (
                    <span className="flex items-center gap-0.5 text-muted-foreground">
                      <GitFork className="h-3 w-3" />
                    </span>
                  )}
                  {s.archived && <Badge variant="destructive">archived</Badge>}
                </div>
                {(s.topics?.length ?? 0) > 0 && (
                  <div className="flex flex-wrap gap-1">
                    {s.topics!.slice(0, 5).map((t) => (
                      <Badge key={t} variant="secondary" className="text-[10px] font-normal">
                        {t}
                      </Badge>
                    ))}
                  </div>
                )}
                {(s.ai_labels?.length ?? 0) > 0 && (
                  <div className="flex flex-wrap gap-1">
                    {s.ai_labels!.map((t) => (
                      <Badge key={t} variant="default" className="text-[10px] font-normal">
                        {t}
                      </Badge>
                    ))}
                  </div>
                )}
              </CardContent>
            </Card>
          ))}
        </div>
      )}
      {moreCursor && (
        <div className="flex justify-center pt-2">
          <Button variant="outline" onClick={loadMore}>
            Load more
          </Button>
        </div>
      )}
    </div>
  )
}
