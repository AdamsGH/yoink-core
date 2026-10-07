import { useCallback, useEffect, useRef, useState, useMemo } from 'react'
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

import { useMutationRecovery } from './useMutationRecovery'
import { continuation } from '../api/star-pagination'

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
  const recovery = useMutationRecovery(setFolders)
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

  const filters = useMemo(() => ({
    limit: PAGE_SIZE,
    language: language || undefined,
    search: debouncedSearch.length >= 2 ? debouncedSearch : undefined,
    folder_id: typeof selectedFolder === 'number' ? selectedFolder : selectedFolder === 'unorganised' ? 0 : undefined,
    sort,
  }), [language, debouncedSearch, selectedFolder, sort])
  const currentFilters = useRef(filters)
  currentFilters.current = filters
  const loadedFilters = useRef<typeof filters | null>(null)
  const generation = useRef(0)
  const moreInFlight = useRef(false)
  const loadStars = useCallback(async () => {
    const version = ++generation.current
    setLoading(true)
    setMoreCursor(null)
    setLoadingMore(false)
    moreInFlight.current = false
    try {
      const res = await listGhStars(filters)
      if (version !== generation.current || currentFilters.current !== filters) return
      loadedFilters.current = filters
      setStars(res.items)
      setMoreCursor(res.next_cursor ?? null)
      setSyncStatus(res.sync_status ?? null)
      setLastSync(res.last_synced_at ?? null)
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      if (version === generation.current && currentFilters.current === filters) toast.error('Failed to load stars')
    } finally {
      if (version === generation.current && currentFilters.current === filters) setLoading(false)
    }
  }, [filters])

  useEffect(() => { void loadFolders() }, [loadFolders])
  useEffect(() => { void loadLanguages() }, [loadLanguages])
  useEffect(() => { void loadStars(); return () => { ++generation.current } }, [loadStars])

  async function loadMore() {
    if (!moreCursor || loading || loadedFilters.current !== filters || moreInFlight.current) return
    const version = generation.current
    moreInFlight.current = true
    setLoadingMore(true)
    try {
      const res = await listGhStars(continuation(filters, moreCursor))
      if (version !== generation.current || currentFilters.current !== filters) return
      setStars((p) => [...p, ...res.items])
      setMoreCursor(res.next_cursor ?? null)
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      if (version === generation.current && currentFilters.current === filters) toast.error('Failed to load more')
    } finally {
      if (version === generation.current && currentFilters.current === filters) {
        moreInFlight.current = false
        setLoadingMore(false)
      }
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
    if (body.is_local === false) return recovery.create(body)
    try {
      const f = await createFolder(body)
      setFolders((p) => [...p, f])
      toast.success('Folder created')
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
      void recovery.refresh()
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
      void recovery.refresh()
    }
  }

  async function onMoveStar(starId: number, targetFolder: FolderSelection) {
    if (movingId === starId) return
    setMovingId(starId)
    const fromFolderId = typeof selectedFolder === 'number' ? selectedFolder : null
    const toFolderId = typeof targetFolder === 'number' ? targetFolder : null
    try {
      if (fromFolderId !== null) {
        await removeStarFromFolder(fromFolderId, starId)
      }
      if (toFolderId !== null) {
        await addStarToFolder(toFolderId, starId)
        toast.success(`Moved to ${folders.find((f) => f.id === toFolderId)?.name ?? 'folder'}`)
      } else {
        toast.success('Removed from folder')
      }
      // Optimistic folder star_count update
      setFolders((prev) => prev.map((f) => {
        if (f.id === fromFolderId) return { ...f, star_count: Math.max(0, f.star_count - 1) }
        if (f.id === toFolderId) return { ...f, star_count: f.star_count + 1 }
        return f
      }))
      // Optimistic folder_ids update on star cards
      setStars((prev) => prev.map((s) => {
        if (s.id !== starId) return s
        const ids = s.folder_ids ?? []
        const next = toFolderId !== null
          ? [...ids.filter((id) => id !== fromFolderId), toFolderId]
          : ids.filter((id) => id !== fromFolderId)
        return { ...s, folder_ids: next }
      }))
      // Remove from view when browsing a specific folder
      if (selectedFolder !== 'all') {
        setStars((p) => p.filter((s) => s.id !== starId))
      }
    } catch {
      toast.error('Failed to move')
      void recovery.refresh()
    } finally {
      setMovingId(null)
    }
  }

  async function onUnstar(starId: number) {
    try {
      await unstarRepo(starId)
      setStars((p) => p.filter((s) => s.id !== starId))
      toast.success('Unstarred')
    } catch {
      toast.error('Failed to unstar')
      void recovery.refresh()
    }
  }

  return {
    recovery, stars, folders, loading, loadingMore, moreCursor,
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
