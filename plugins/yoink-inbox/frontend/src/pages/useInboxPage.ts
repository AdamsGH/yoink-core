import { useCallback, useEffect, useRef, useState } from 'react'

import {
  deleteItem,
  listCategories,
  listItems,
  reclassifyItem,
  setItemCategories,
} from '@inbox/api/items'
import type { InboxCategory, InboxItem } from '@inbox/types'

export type CategorySelection = 'all' | 'uncategorized' | number

export interface UseInboxPage {
  // data
  items: InboxItem[]
  categories: InboxCategory[]
  loading: boolean
  loadingMore: boolean
  moreCursor: string | null

  // selection / filters
  selectedCategory: CategorySelection
  setSelectedCategory: (c: CategorySelection) => void
  search: string
  setSearch: (s: string) => void
  status: string
  setStatus: (s: string) => void

  // detail drawer
  openItem: InboxItem | null
  setOpenItem: (item: InboxItem | null) => void

  // drag target
  draggingOver: CategorySelection | null
  setDraggingOver: (c: CategorySelection | null) => void

  // actions
  loadMore: () => Promise<void>
  onAssignCategory: (itemId: number, categoryId: number | null) => Promise<void>
  onReclassify: (itemId: number) => Promise<void>
  onDelete: (itemId: number) => Promise<void>
  onCreateCategory: (name: string) => Promise<void>
}

const PAGE_SIZE = 40

export function useInboxPage(): UseInboxPage {
  const [items, setItems] = useState<InboxItem[]>([])
  const [categories, setCategories] = useState<InboxCategory[]>([])
  const [loading, setLoading] = useState(true)
  const [loadingMore, setLoadingMore] = useState(false)
  const [moreCursor, setMoreCursor] = useState<string | null>(null)

  const [selectedCategory, setSelectedCategory] = useState<CategorySelection>('all')
  const [search, setSearch] = useState('')
  const [status, setStatus] = useState('')
  const [openItem, setOpenItem] = useState<InboxItem | null>(null)
  const [draggingOver, setDraggingOver] = useState<CategorySelection | null>(null)

  const searchTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const [debouncedSearch, setDebouncedSearch] = useState('')

  useEffect(() => {
    if (searchTimer.current) clearTimeout(searchTimer.current)
    searchTimer.current = setTimeout(() => setDebouncedSearch(search), 300)
    return () => { if (searchTimer.current) clearTimeout(searchTimer.current) }
  }, [search])

  const loadCategories = useCallback(async () => {
    try {
      const data = await listCategories()
      setCategories(data)
    } catch { /* ignore */ }
  }, [])

  const loadItems = useCallback(async () => {
    setLoading(true)
    try {
      const resp = await listItems({
        search: debouncedSearch || undefined,
        status: status || undefined,
        category_id: typeof selectedCategory === 'number' ? selectedCategory : undefined,
        uncategorized: selectedCategory === 'uncategorized' ? true : undefined,
        limit: PAGE_SIZE,
      })
      setItems(resp.items)
      setMoreCursor(resp.next_cursor)
    } catch { /* ignore */ } finally {
      setLoading(false)
    }
  }, [debouncedSearch, status, selectedCategory])

  useEffect(() => { void loadCategories() }, [loadCategories])
  useEffect(() => { void loadItems() }, [loadItems])

  const loadMore = useCallback(async () => {
    if (!moreCursor || loadingMore) return
    setLoadingMore(true)
    try {
      const resp = await listItems({
        search: debouncedSearch || undefined,
        status: status || undefined,
        category_id: typeof selectedCategory === 'number' ? selectedCategory : undefined,
        uncategorized: selectedCategory === 'uncategorized' ? true : undefined,
        limit: PAGE_SIZE,
        cursor: moreCursor,
      })
      setItems((prev) => [...prev, ...resp.items])
      setMoreCursor(resp.next_cursor)
    } catch { /* ignore */ } finally {
      setLoadingMore(false)
    }
  }, [moreCursor, loadingMore, debouncedSearch, status, selectedCategory])

  const onAssignCategory = useCallback(async (itemId: number, categoryId: number | null) => {
    const ids = categoryId !== null ? [categoryId] : []
    await setItemCategories(itemId, ids)
    // Update local state
    setItems((prev) =>
      prev.map((item) => {
        if (item.id !== itemId) return item
        if (categoryId === null) {
          return { ...item, categories: [] }
        }
        const cat = categories.find((c) => c.id === categoryId)
        if (!cat) return item
        const already = item.categories.find((c) => c.id === categoryId)
        if (already) return item
        return {
          ...item,
          categories: [
            ...item.categories,
            { id: cat.id, name: cat.name, slug: cat.slug, color: cat.color, attached_by: 'user' as const, confidence: null },
          ],
        }
      }),
    )
    await loadCategories()
  }, [categories, loadCategories])

  const onReclassify = useCallback(async (itemId: number) => {
    await reclassifyItem(itemId)
    // Optimistic: mark as pending
    setItems((prev) =>
      prev.map((item) => item.id === itemId ? { ...item, llm_status: 'pending' } : item),
    )
  }, [])

  const onDelete = useCallback(async (itemId: number) => {
    await deleteItem(itemId)
    setItems((prev) => prev.filter((item) => item.id !== itemId))
    if (openItem?.id === itemId) setOpenItem(null)
    await loadCategories()
  }, [openItem, loadCategories])

  const onCreateCategory = useCallback(async (name: string) => {
    const { createCategory } = await import('@inbox/api/items')
    await createCategory({ name })
    await loadCategories()
  }, [loadCategories])

  return {
    items, categories, loading, loadingMore, moreCursor,
    selectedCategory, setSelectedCategory,
    search, setSearch,
    status, setStatus,
    openItem, setOpenItem,
    draggingOver, setDraggingOver,
    loadMore,
    onAssignCategory, onReclassify, onDelete, onCreateCategory,
  }
}
