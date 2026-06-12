import { useCallback, useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'

import {
  addStarToFolder,
  createFolder,
  deleteFolder,
  listFolders,
  listGhStars,
  listStarLanguages,
  patchFolder,
  removeStarFromFolder,
  triggerGhSync,
  unstarRepo,
  updateFolder,
} from '@inbox/api/items'
import type { GhStarSort } from '@inbox/api/items'
import type { InboxGhFolder, InboxGhFolderCreate, InboxGhStar } from '@inbox/types'

export type FolderSelection = 'all' | 'unorganised' | number

const PAGE_SIZE = 30

export function useStarsPage() {
  const [stars, setStars] = useState<InboxGhStar[]>([])
  const [folders, setFolders] = useState<InboxGhFolder[]>([])
  const [loading, setLoading] = useState(true)
  const [loadingMore, setLoadingMore] = useState(false)
  const [moreCursor, setMoreCursor] = useState<string | null>(null)
  const [syncStatus, setSyncStatus] = useState<string | null>(null)
  const [lastSync, setLastSync] = useState<string | null>(null)
  const [search, setSearch] = useState('')
  const [debouncedSearch, setDebouncedSearch] = useState('')
  const [language, setLanguage] = useState('')
  const [sort, setSort] = useState<GhStarSort>('starred_at')
  const [selectedFolder, setSelectedFolder] = useState<FolderSelection>('all')
  const [movingId, setMovingId] = useState<number | null>(null)
  const [languages, setLanguages] = useState<string[]>([])
  const debounceTimer = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    if (debounceTimer.current) clearTimeout(debounceTimer.current)
    debounceTimer.current = setTimeout(() => setDebouncedSearch(search), 350)
    return () => { if (debounceTimer.current) clearTimeout(debounceTimer.current) }
  }, [search])

  const loadFolders = useCallback(async () => {
    try {
      setFolders(await listFolders())
    } catch { /* non-critical */ }
  }, [])

  const loadLanguages = useCallback(async () => {
    try {
      setLanguages(await listStarLanguages())
    } catch { /* non-critical */ }
  }, [])

  const loadStars = useCallback(async () => {
    setLoading(true)
    try {
      const res = await listGhStars({
        limit: PAGE_SIZE,
        language: language || undefined,
        search: debouncedSearch.length >= 2 ? debouncedSearch : undefined,
        folder_id:
          typeof selectedFolder === 'number' ? selectedFolder :
          selectedFolder === 'unorganised' ? 0 :
          undefined,
        sort,
      })
      setStars(res.items)
      setMoreCursor(res.next_cursor ?? null)
      setSyncStatus(res.sync_status ?? null)
      setLastSync(res.last_synced_at ?? null)
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to load stars')
    } finally {
      setLoading(false)
    }
  }, [language, debouncedSearch, selectedFolder, sort])

  useEffect(() => { void loadFolders() }, [loadFolders])
  useEffect(() => { void loadLanguages() }, [loadLanguages])
  useEffect(() => { void loadStars() }, [loadStars])

  async function loadMore() {
    if (!moreCursor || loadingMore) return
    setLoadingMore(true)
    try {
      const isOffset = sort !== 'starred_at'
      const res = await listGhStars({
        limit: PAGE_SIZE,
        ...(isOffset
          ? { offset: Number(moreCursor) }
          : { cursor: moreCursor }),
        language: language || undefined,
        search: debouncedSearch.length >= 2 ? debouncedSearch : undefined,
        sort,
      })
      setStars((p) => [...p, ...res.items])
      setMoreCursor(res.next_cursor ?? null)
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to load more')
    } finally {
      setLoadingMore(false)
    }
  }

  async function onSync() {
    try {
      const r = await triggerGhSync()
      toast.success(`Sync ${r.status}`)
    } catch {
      toast.error('Failed to enqueue sync')
    }
  }

  async function onCreateFolder(body: InboxGhFolderCreate) {
    try {
      const f = await createFolder(body)
      setFolders((p) => [...p, f])
      toast.success(body.is_local === false ? 'Folder created and synced to GitHub Lists' : 'Folder created')
      return f
    } catch {
      toast.error('Failed to create folder')
      return null
    }
  }

  async function onPinFolder(id: number, pinned: boolean) {
    try {
      const f = await patchFolder(id, { is_pinned: pinned })
      setFolders((p) => p.map((x) => (x.id === id ? f : x)))
    } catch {
      toast.error('Failed to update folder')
    }
  }

  async function onReorderPinned(orderedIds: number[]) {
    // optimistic update
    setFolders((prev) => {
      const map = new Map(prev.map((f) => [f.id, f]))
      return prev.map((f) => {
        const idx = orderedIds.indexOf(f.id)
        return idx >= 0 ? { ...f, sort_order: idx } : f
      }).sort((a, b) => {
        if (a.is_pinned && b.is_pinned) return a.sort_order - b.sort_order
        if (a.is_pinned) return -1
        if (b.is_pinned) return 1
        return a.name.localeCompare(b.name)
      })
      void map
    })
    // persist each changed order
    await Promise.all(
      orderedIds.map((id, idx) => patchFolder(id, { sort_order: idx }).catch(() => null))
    )
  }

  async function onRenameFolder(id: number, body: InboxGhFolderCreate) {
    try {
      const f = await updateFolder(id, body)
      setFolders((p) => p.map((x) => (x.id === id ? f : x)))
    } catch {
      toast.error('Failed to rename')
    }
  }

  async function onDeleteFolder(id: number) {
    try {
      await deleteFolder(id)
      setFolders((p) => p.filter((x) => x.id !== id))
      if (selectedFolder === id) setSelectedFolder('all')
      toast.success('Folder deleted')
    } catch {
      toast.error('Failed to delete')
    }
  }

  async function onMoveStar(starId: number, targetFolder: FolderSelection) {
    if (movingId === starId) return
    setMovingId(starId)
    try {
      if (typeof selectedFolder === 'number') {
        await removeStarFromFolder(selectedFolder, starId)
      }
      if (typeof targetFolder === 'number') {
        await addStarToFolder(targetFolder, starId)
        toast.success(`Moved to ${folders.find((f) => f.id === targetFolder)?.name ?? 'folder'}`)
      } else {
        toast.success('Removed from folder')
      }
      if (selectedFolder !== 'all') {
        setStars((p) => p.filter((s) => s.id !== starId))
      }
    } catch {
      toast.error('Failed to move')
    } finally {
      setMovingId(null)
    }
  }

  async function onUnstar(starId: number, repoId: number) {
    try {
      await unstarRepo(repoId)
      setStars((p) => p.filter((s) => s.id !== starId))
      toast.success('Unstarred')
    } catch {
      toast.error('Failed to unstar')
    }
  }

  return {
    stars, folders, loading, loadingMore, moreCursor,
    syncStatus, lastSync,
    search, setSearch,
    language, setLanguage,
    sort, setSort,
    selectedFolder, setSelectedFolder,
    movingId,
    loadMore, onSync,
    onCreateFolder, onRenameFolder, onDeleteFolder,
    onPinFolder, onReorderPinned,
    languages,
    onMoveStar, onUnstar,
    reload: loadStars,
  }
}
