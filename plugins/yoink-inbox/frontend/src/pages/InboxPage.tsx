import {
  DndContext,
  DragOverlay,
  PointerSensor,
  useDraggable,
  useDroppable,
  useSensor,
  useSensors,
} from '@dnd-kit/core'
import type { DragEndEvent, DragStartEvent } from '@dnd-kit/core'
import {
  ExternalLink,
  FolderOpen,
  Inbox,
  LayoutGrid,
  LayoutList,
  Plus,
  RefreshCw,
  Search,
  Tag,
  Trash2,
  X,
} from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { snapCenterToCursor } from '@dnd-kit/modifiers'
import { pointerWithin } from '@dnd-kit/core'

import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
  Button,
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
  Input,
  Skeleton,
  ToggleGroup,
  ToggleGroupItem,
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@ui'
import { cn } from '@core/lib/utils'
import { useRightSidebar } from '@core/layout/AppLayout'
import { BlurDialog, BlurDialogContent, BlurDialogHeader, BlurDialogBody, BlurDialogFooter } from '@app'
import { Popover, PopoverContent, PopoverTrigger } from '@ui'

// ---------------------------------------------------------------------------
// Reusable droppable zone chip (used in sidebar + mobile dock)
// ---------------------------------------------------------------------------

function CategoryDropTarget({
  droppableId, label, isOver, icon,
}: {
  droppableId: string
  label: string
  isOver: boolean
  icon: React.ReactNode
}) {
  const { setNodeRef } = useDroppable({ id: droppableId })
  return (
    <div
      ref={setNodeRef}
      className={cn(
        'flex items-center gap-1.5 px-3 py-2 rounded-lg border text-xs font-medium shrink-0 transition-all duration-150',
        isOver
          ? 'bg-primary text-primary-foreground border-primary scale-105 shadow-md'
          : 'bg-muted/40 text-muted-foreground border-border/50 hover:bg-muted',
      )}
    >
      {icon}
      <span>{label}</span>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Shared category badge
// ---------------------------------------------------------------------------

function CategoryBadge({
  name,
  onRemove,
}: {
  name: string
  onRemove?: () => void
}) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 text-[10px] px-2 py-0.5 rounded-full',
        'bg-primary/10 text-primary border border-primary/20 font-medium',
        onRemove && 'cursor-pointer hover:bg-destructive/10 hover:text-destructive hover:border-destructive/20 transition-colors',
      )}
      onClick={onRemove}
    >
      <span className="max-w-[8rem] truncate">{name}</span>
      {onRemove && <X className="w-2.5 h-2.5 shrink-0" />}
    </span>
  )
}

// ---------------------------------------------------------------------------
// Category sidebar content (mounted into right sidebar via useRightSidebar)
// ---------------------------------------------------------------------------

interface CategorySidebarProps {
  items: ReturnType<typeof useInboxPage>['items']
  categories: ReturnType<typeof useInboxPage>['categories']
  selectedCategory: ReturnType<typeof useInboxPage>['selectedCategory']
  draggingOver: ReturnType<typeof useInboxPage>['draggingOver']
  setSelectedCategory: ReturnType<typeof useInboxPage>['setSelectedCategory']
  onCreateCategory: ReturnType<typeof useInboxPage>['onCreateCategory']
}

function CategorySidebar({
  items, categories, selectedCategory, draggingOver, setSelectedCategory, onCreateCategory,
}: CategorySidebarProps) {
  const [newCatName, setNewCatName] = useState('')
  const [creatingCat, setCreatingCat] = useState(false)
  const [showNewCat, setShowNewCat] = useState(false)

  async function handleCreate() {
    if (!newCatName.trim() || creatingCat) return
    setCreatingCat(true)
    try { await onCreateCategory(newCatName.trim()); setNewCatName(''); setShowNewCat(false) }
    finally { setCreatingCat(false) }
  }

  return (
    <div className="flex flex-col h-full">
      <div className="px-3 pt-4 pb-2">
        <p className="text-[11px] font-semibold text-muted-foreground uppercase tracking-widest px-2 mb-2">Categories</p>
      </div>
      <div className="flex-1 overflow-y-auto px-2">
        <div className="flex flex-col gap-0.5 pb-3">
          <CategoryNode droppableId="all" label="All Items" count={items.length}
            selected={selectedCategory === 'all'} isOver={draggingOver === 'all'}
            icon={<Inbox className="w-3.5 h-3.5" />} onClick={() => setSelectedCategory('all')} />
          <CategoryNode droppableId="uncategorized" label="Uncategorized"
            selected={selectedCategory === 'uncategorized'} isOver={draggingOver === 'uncategorized'}
            icon={<Tag className="w-3.5 h-3.5" />} onClick={() => setSelectedCategory('uncategorized')} />
          {categories.length > 0 && <div className="mx-2 my-2 border-t border-border/50" />}
          {categories.map((cat) => (
            <CategoryNode key={cat.id} droppableId={`cat-${cat.id}`} label={cat.name}
              count={cat.item_count} selected={selectedCategory === cat.id}
              isOver={draggingOver === cat.id}
              icon={<FolderOpen className="w-3.5 h-3.5" />}
              onClick={() => setSelectedCategory(cat.id)} />
          ))}
        </div>
      </div>
      <div className="px-2 pb-3 border-t border-border/40 pt-2">
        {showNewCat ? (
          <div className="flex gap-1">
            <Input value={newCatName} onChange={(e) => setNewCatName(e.target.value)}
              placeholder="Category name..." className="h-7 text-xs"
              onKeyDown={(e) => {
                if (e.key === 'Enter') void handleCreate()
                if (e.key === 'Escape') { setShowNewCat(false); setNewCatName('') }
              }} autoFocus disabled={creatingCat} />
            <Button size="icon" className="h-7 w-7 shrink-0" onClick={handleCreate} disabled={creatingCat}>
              <Plus className="w-3.5 h-3.5" />
            </Button>
          </div>
        ) : (
          <button onClick={() => setShowNewCat(true)}
            className="w-full flex items-center gap-2 px-2.5 py-1.5 rounded-md text-xs text-muted-foreground hover:text-foreground hover:bg-accent transition-colors">
            <Plus className="w-3.5 h-3.5" /> New category
          </button>
        )}
      </div>
    </div>
  )
}

import type { InboxCategory, InboxItem } from '@inbox/types'
import { useInboxPage } from './useInboxPage'

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function timeAgo(iso: string): string {
  const diff = Date.now() - new Date(iso).getTime()
  const m = Math.floor(diff / 60000)
  if (m < 1) return 'just now'
  if (m < 60) return `${m}m ago`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h}h ago`
  return `${Math.floor(h / 24)}d ago`
}

function statusBadgeClass(s: string): string {
  if (s === 'classified') return 'bg-emerald-500/15 text-emerald-400 border-emerald-500/25'
  if (s === 'enriched') return 'bg-sky-500/15 text-sky-400 border-sky-500/25'
  if (s === 'pending') return 'bg-amber-500/15 text-amber-400 border-amber-500/25'
  if (s === 'failed' || s === 'llm:failed') return 'bg-red-500/15 text-red-400 border-red-500/25'
  return 'bg-muted/60 text-muted-foreground border-transparent'
}

// ---------------------------------------------------------------------------
// Droppable category node
// ---------------------------------------------------------------------------

interface CategoryNodeProps {
  droppableId: string
  label: string
  count?: number
  selected: boolean
  isOver: boolean
  icon: React.ReactNode
  onClick: () => void
}

function CategoryNode({ droppableId, label, count, selected, isOver, icon, onClick }: CategoryNodeProps) {
  const { setNodeRef } = useDroppable({ id: droppableId })
  return (
    <button
      ref={setNodeRef}
      onClick={onClick}
      className={cn(
        'w-full flex items-center gap-2 px-2.5 py-1.5 rounded-md text-sm transition-all duration-100 text-left',
        selected && !isOver && 'bg-primary/15 text-primary font-medium',
        !selected && !isOver && 'text-muted-foreground hover:bg-accent hover:text-foreground',
        isOver && 'bg-primary/25 text-primary ring-1 ring-primary/40 scale-[1.01]',
      )}
    >
      <span className="shrink-0">{icon}</span>
      <span className="flex-1 truncate">{label}</span>
      {count !== undefined && (
        <span className="text-[11px] tabular-nums text-muted-foreground/70 font-mono">{count}</span>
      )}
    </button>
  )
}

// ---------------------------------------------------------------------------
// Draggable item row with ContextMenu
// ---------------------------------------------------------------------------

interface ItemRowProps {
  item: InboxItem
  view: 'list' | 'grid'
  isDragging: boolean
  categories: InboxCategory[]
  onOpen: (item: InboxItem) => void
  onAssign: (itemId: number, catId: number | null) => void
  onReclassify: (id: number) => void
  onDelete: (id: number) => void
}

function ItemRow({ item, view, isDragging, categories, onOpen, onAssign, onReclassify, onDelete }: ItemRowProps) {
  const { attributes, listeners, setNodeRef, transform } = useDraggable({
    id: `item-${item.id}`,
    data: { itemId: item.id },
  })
  // When dragging: hide the original (opacity-0) and let DragOverlay do the
  // visual. Don't apply transform to the source - it creates overflow and
  // shifts layout. The overlay handles positioning.
  const style = undefined
  // Suppress click after drag: track whether pointer moved more than threshold
  const didDragRef = useRef(false)
  useEffect(() => {
    if (transform) { didDragRef.current = true }
    else { setTimeout(() => { didDragRef.current = false }, 50) }
  }, [transform])

  const displayStatus = item.llm_status === 'failed' ? 'llm:failed' : item.llm_status ?? item.status

  const isGrid = view === 'grid'

  const row = (
    <div
      ref={setNodeRef}
      style={style}
      {...attributes}
      {...listeners}
      onClick={() => { if (!didDragRef.current) onOpen(item) }}
      className={cn(
        'group relative flex cursor-grab active:cursor-grabbing transition-all duration-150 min-w-0',
        'rounded-xl border border-border/50 bg-card hover:border-primary/40 hover:shadow-md hover:shadow-black/20',
        isGrid ? 'flex-col overflow-hidden' : 'flex-row items-start gap-4 p-3',
        isDragging && 'opacity-0',
      )}
    >
      {/* Thumbnail */}
      {isGrid ? (
        <div className="w-full h-36 bg-muted/40 overflow-hidden border-b border-border/40 shrink-0">
          {item.og_image_url ? (
            <img src={item.og_image_url} alt="" className="w-full h-full object-cover"
              onError={(e) => { (e.target as HTMLImageElement).style.display = 'none' }} />
          ) : (
            <div className="w-full h-full flex items-center justify-center">
              <Inbox className="w-8 h-8 text-muted-foreground/20" />
            </div>
          )}
        </div>
      ) : (
        <div className="shrink-0 w-10 h-10 rounded-lg bg-muted/50 border border-border/40 overflow-hidden mt-0.5">
          {item.og_image_url ? (
            <img src={item.og_image_url} alt="" className="w-full h-full object-cover"
              onError={(e) => { (e.target as HTMLImageElement).style.display = 'none' }} />
          ) : (
            <div className="w-full h-full flex items-center justify-center">
              <Inbox className="w-4 h-4 text-muted-foreground/30" />
            </div>
          )}
        </div>
      )}

      {/* Content */}
      <div className={cn('flex flex-col min-w-0 flex-1', isGrid ? 'p-3 gap-1' : 'gap-1')}>
        <div className="flex items-start justify-between gap-2 min-w-0">
          <p className="font-semibold text-sm leading-snug truncate flex-1 min-w-0">
            {item.title || item.url}
          </p>
          <span className="shrink-0 text-[11px] text-muted-foreground/50 tabular-nums pt-px whitespace-nowrap">
            {timeAgo(item.created_at)}
          </span>
        </div>

        {item.summary && (
          <p className={cn('text-xs text-muted-foreground leading-relaxed', isGrid ? 'line-clamp-3' : 'line-clamp-1')}>
            {item.summary}
          </p>
        )}

        <div className="flex flex-wrap gap-1 mt-1 overflow-hidden">
          <span className={cn('text-[10px] px-1.5 py-0.5 rounded-md border font-mono shrink-0', statusBadgeClass(displayStatus))}>
            {displayStatus}
          </span>
          <span className="text-[10px] px-1.5 py-0.5 rounded-md border border-border/40 bg-muted/30 text-muted-foreground font-mono shrink-0">
            {item.kind}
          </span>
          {item.categories.map((cat) => (
            <CategoryBadge key={cat.id} name={cat.name} />
          ))}
        </div>
      </div>

      {/* Hover actions */}
      <div className={cn(
        'absolute top-2 right-2 flex items-center gap-0.5',
        'opacity-0 group-hover:opacity-100 transition-opacity',
      )}>
        <TooltipProvider>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button variant="secondary" size="icon" className="h-6 w-6"
                onClick={(e) => { e.stopPropagation(); window.open(item.url, '_blank') }}>
                <ExternalLink className="w-3 h-3" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom">Open URL</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button variant="secondary" size="icon" className="h-6 w-6"
                onClick={(e) => { e.stopPropagation(); onReclassify(item.id) }}>
                <RefreshCw className="w-3 h-3" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom">Reclassify</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button variant="secondary" size="icon" className="h-6 w-6 hover:text-destructive"
                onClick={(e) => { e.stopPropagation(); onDelete(item.id) }}>
                <Trash2 className="w-3 h-3" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom">Delete</TooltipContent>
          </Tooltip>
        </TooltipProvider>
      </div>
    </div>
  )

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild className="block min-w-0">{row}</ContextMenuTrigger>
      <ContextMenuContent className="w-52">
        <ContextMenuItem onSelect={() => window.open(item.url, '_blank')}>
          <ExternalLink className="w-4 h-4 mr-2" /> Open URL
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuSub>
          <ContextMenuSubTrigger>
            <Tag className="w-4 h-4 mr-2" /> Assign category
          </ContextMenuSubTrigger>
          <ContextMenuSubContent className="w-48">
            <ContextMenuItem onSelect={() => onAssign(item.id, null)}>
              <X className="w-3.5 h-3.5 mr-2 text-muted-foreground" /> Clear categories
            </ContextMenuItem>
            <ContextMenuSeparator />
            {categories.map((cat) => (
              <ContextMenuItem key={cat.id} onSelect={() => onAssign(item.id, cat.id)}>
                <FolderOpen className="w-3.5 h-3.5 mr-2 text-muted-foreground" />
                {cat.name}
                {item.categories.find((c) => c.id === cat.id) && (
                  <span className="ml-auto text-primary">✓</span>
                )}
              </ContextMenuItem>
            ))}
          </ContextMenuSubContent>
        </ContextMenuSub>
        <ContextMenuItem onSelect={() => onReclassify(item.id)}>
          <RefreshCw className="w-4 h-4 mr-2" /> Reclassify
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem
          onSelect={() => onDelete(item.id)}
          className="text-destructive focus:text-destructive"
        >
          <Trash2 className="w-4 h-4 mr-2" /> Delete
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  )
}

// ---------------------------------------------------------------------------
// Item detail sheet
// ---------------------------------------------------------------------------

function ItemSheet({
  item,
  categories,
  onClose,
  onAssign,
  onReclassify,
  onDelete,
}: {
  item: InboxItem
  categories: InboxCategory[]
  onClose: () => void
  onAssign: (catId: number | null) => void
  onReclassify: () => void
  onDelete: () => void
}) {
  const displayStatus = item.llm_status === 'failed' ? 'llm:failed' : item.llm_status ?? item.status

  return (
    <BlurDialog open onOpenChange={(o) => !o && onClose()}>
      <BlurDialogContent className="max-w-xl">
        <BlurDialogHeader
          title={item.title || item.url}
          description={
            <a href={item.url} target="_blank" rel="noopener noreferrer"
              className="text-xs text-muted-foreground hover:text-primary truncate block">
              {item.url}
            </a>
          }
        />

        <BlurDialogBody className="space-y-5 p-0">
          {item.og_image_url && (
            <img src={item.og_image_url} alt=""
              className="w-full h-48 object-cover border-b border-border/60" />
          )}

          <div className="px-5 pt-4 space-y-5">
            <div className="flex items-center gap-2 flex-wrap">
              <span className={cn('text-xs px-2 py-0.5 rounded border font-mono', statusBadgeClass(displayStatus))}>
                {displayStatus}
              </span>
              <span className="text-xs px-2 py-0.5 rounded bg-muted/40 text-muted-foreground font-mono">
                {item.kind}
              </span>
              <span className="text-xs text-muted-foreground ml-auto">{timeAgo(item.created_at)}</span>
            </div>

            {item.summary && (
              <div>
                <p className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground mb-1.5">Summary</p>
                <p className="text-sm leading-relaxed">{item.summary}</p>
              </div>
            )}

            <div className="pb-2">
              <p className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground mb-2">Categories</p>
              <div className="flex flex-wrap gap-1.5 items-center">
                {item.categories.length === 0 && (
                  <span className="text-xs text-muted-foreground">No categories assigned</span>
                )}
                {item.categories.map((cat) => (
                  <CategoryBadge key={cat.id} name={cat.name} onRemove={() => onAssign(null)} />
                ))}
                {categories.filter((c) => !item.categories.find((ic) => ic.id === c.id)).length > 0 && (
                  <Popover>
                    <PopoverTrigger asChild>
                      <Button variant="outline" size="sm" className="h-6 px-2 text-xs gap-1">
                        <Plus className="w-3 h-3" /> Add
                      </Button>
                    </PopoverTrigger>
                    <PopoverContent className="w-48 p-1" align="start">
                      <div className="flex flex-col gap-0.5">
                        {categories
                          .filter((c) => !item.categories.find((ic) => ic.id === c.id))
                          .map((cat) => (
                            <button
                              key={cat.id}
                              onClick={() => onAssign(cat.id)}
                              className="flex items-center gap-2 px-2 py-1.5 text-sm rounded hover:bg-accent text-left w-full transition-colors"
                            >
                              <FolderOpen className="w-3.5 h-3.5 text-muted-foreground shrink-0" />
                              {cat.name}
                            </button>
                          ))}
                      </div>
                    </PopoverContent>
                  </Popover>
                )}
              </div>
            </div>
          </div>
        </BlurDialogBody>

        <BlurDialogFooter>
          <Button variant="outline" size="sm" onClick={onReclassify} className="gap-1.5">
            <RefreshCw className="w-3.5 h-3.5" /> Reclassify
          </Button>
          <Button asChild variant="outline" size="sm" className="gap-1.5">
            <a href={item.url} target="_blank" rel="noopener noreferrer">
              <ExternalLink className="w-3.5 h-3.5" /> Open
            </a>
          </Button>
          <Button variant="outline" size="sm"
            className="gap-1.5 ml-auto text-destructive hover:bg-destructive/10 hover:border-destructive/30"
            onClick={onDelete}>
            <Trash2 className="w-3.5 h-3.5" /> Delete
          </Button>
        </BlurDialogFooter>
      </BlurDialogContent>
    </BlurDialog>
  )
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

export default function InboxPage() {
  const {
    items, categories, loading, loadingMore, moreCursor,
    selectedCategory, setSelectedCategory,
    search, setSearch, status, setStatus,
    openItem, setOpenItem,
    draggingOver, setDraggingOver,
    loadMore,
    onAssignCategory, onReclassify, onDelete, onCreateCategory,
  } = useInboxPage()

  const { setContent, openMobileSheet } = useRightSidebar()
  const [activeItemId, setActiveItemId] = useState<number | null>(null)
  const [view, setView] = useState<'list' | 'grid'>(() => {
    if (typeof window === 'undefined') return 'list'
    const stored = window.localStorage.getItem('inbox:view')
    return stored === 'grid' || stored === 'list' ? stored : 'list'
  })
  useEffect(() => {
    if (typeof window !== 'undefined') window.localStorage.setItem('inbox:view', view)
  }, [view])

  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 8 } }))
  const activeItem = items.find((i) => i.id === activeItemId) ?? null
  const dockScrollRef = useRef<HTMLDivElement>(null)
  const dockScrollRaf = useRef<number | null>(null)

  // Dock edge-scroll: listen to raw pointermove on window while dragging.
  // Runs a rAF loop that keeps scrolling as long as pointer is near edge.
  useEffect(() => {
    if (!activeItemId) {
      if (dockScrollRaf.current) { cancelAnimationFrame(dockScrollRaf.current); dockScrollRaf.current = null }
      return
    }
    let pointerX = 0
    const onMove = (e: PointerEvent) => { pointerX = e.clientX }
    window.addEventListener('pointermove', onMove, { passive: true })
    const EDGE = 60
    const SPEED = 10
    const tick = () => {
      const dock = dockScrollRef.current
      if (dock) {
        const rect = dock.getBoundingClientRect()
        const inDockY = pointerX >= rect.left && pointerX <= rect.right
        if (inDockY) {
          if (pointerX < rect.left + EDGE) dock.scrollLeft -= SPEED
          else if (pointerX > rect.right - EDGE) dock.scrollLeft += SPEED
        }
      }
      dockScrollRaf.current = requestAnimationFrame(tick)
    }
    dockScrollRaf.current = requestAnimationFrame(tick)
    return () => {
      window.removeEventListener('pointermove', onMove)
      if (dockScrollRaf.current) { cancelAnimationFrame(dockScrollRaf.current); dockScrollRaf.current = null }
    }
  }, [activeItemId])

  useEffect(() => {
    setContent(
      <CategorySidebar
        items={items}
        categories={categories}
        selectedCategory={selectedCategory}
        draggingOver={draggingOver}
        setSelectedCategory={setSelectedCategory}
        onCreateCategory={onCreateCategory}
      />
    )
    return () => setContent(null)
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [setContent, items.length, categories, selectedCategory, draggingOver])

  function handleDragStart(e: DragStartEvent) {
    setActiveItemId((e.active.data.current as { itemId: number }).itemId)
  }

  async function handleDragEnd(e: DragEndEvent) {
    setActiveItemId(null)
    setDraggingOver(null)
    const itemId = (e.active.data.current as { itemId: number }).itemId
    const overId = e.over?.id as string | undefined
    if (!overId) return
    if (overId.startsWith('cat-')) {
      await onAssignCategory(itemId, parseInt(overId.replace('cat-', ''), 10))
    } else if (overId === 'uncategorized') {
      await onAssignCategory(itemId, null)
    }
  }

  function handleDragOver(e: { over: { id: string } | null }) {
    if (!e.over) { setDraggingOver(null); return }
    const id = e.over.id as string
    if (id === 'all') setDraggingOver('all')
    else if (id === 'uncategorized') setDraggingOver('uncategorized')
    else if (id.startsWith('cat-')) setDraggingOver(parseInt(id.replace('cat-', ''), 10))
    else setDraggingOver(null)
  }

  const selectedCatLabel = selectedCategory === 'all'
    ? 'All Items'
    : selectedCategory === 'uncategorized'
      ? 'Uncategorized'
      : categories.find((c) => c.id === selectedCategory)?.name ?? 'Items'

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={pointerWithin}
      onDragStart={handleDragStart}
      onDragEnd={handleDragEnd}
      onDragOver={handleDragOver as never}
      autoScroll={{
          threshold: { x: 0.1, y: 0.2 },
          // Only allow autoscroll on the dock element, not the document
          canScroll: (element) => element === dockScrollRef.current,
        }}
    >
      <div className="flex flex-col h-full min-w-0 overflow-x-clip">
            {/* Toolbar */}
            <div className="flex flex-wrap items-center gap-2 px-4 py-2.5 border-b border-border/60 shrink-0 min-w-0">
              <Breadcrumb className="min-w-0">
                <BreadcrumbList>
                  <BreadcrumbItem>
                    <BreadcrumbLink className="text-muted-foreground hover:text-foreground text-sm cursor-pointer"
                      onClick={() => setSelectedCategory('all')}>
                      Inbox
                    </BreadcrumbLink>
                  </BreadcrumbItem>
                  {selectedCategory !== 'all' && (
                    <>
                      <BreadcrumbSeparator />
                      <BreadcrumbItem>
                        <BreadcrumbPage className="text-sm">{selectedCatLabel}</BreadcrumbPage>
                      </BreadcrumbItem>
                    </>
                  )}
                </BreadcrumbList>
              </Breadcrumb>

              <div className="flex-1 min-w-0" />

              <div className="relative flex-1 min-w-[140px] max-w-xs">
                <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-muted-foreground pointer-events-none" />
                <Input
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                  placeholder="Search..."
                  className="pl-8 h-8 text-sm w-full"
                />
              </div>

              <select
                value={status}
                onChange={(e) => setStatus(e.target.value)}
                className="h-8 rounded-md border border-input bg-background px-2.5 text-sm text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring shrink-0"
              >
                <option value="">All statuses</option>
                <option value="pending">Pending</option>
                <option value="enriched">Enriched</option>
                <option value="classified">Classified</option>
                <option value="failed">Failed</option>
              </select>

              <Button variant="outline" size="icon" className="h-8 w-8 shrink-0 md:hidden" onClick={openMobileSheet}>
                <FolderOpen className="w-3.5 h-3.5" />
              </Button>

              <ToggleGroup type="single" value={view} onValueChange={(v) => v && setView(v as 'list' | 'grid')} className="h-8 shrink-0">
                <ToggleGroupItem value="list" className="h-8 w-8 p-0">
                  <LayoutList className="w-3.5 h-3.5" />
                </ToggleGroupItem>
                <ToggleGroupItem value="grid" className="h-8 w-8 p-0">
                  <LayoutGrid className="w-3.5 h-3.5" />
                </ToggleGroupItem>
              </ToggleGroup>

              <span className="text-xs text-muted-foreground tabular-nums shrink-0">{items.length} items</span>
            </div>

            {/* Mobile drag dock - inline under toolbar, visible only while dragging */}
            {activeItem && (
              <div className="md:hidden shrink-0 border-b border-border/60 bg-muted/20">
                <div ref={dockScrollRef} className="flex gap-2 px-3 py-2 overflow-x-auto">
                  <CategoryDropTarget droppableId="uncategorized" label="Uncategorized"
                    isOver={draggingOver === 'uncategorized'}
                    icon={<Tag className="w-3.5 h-3.5" />} />
                  {categories.map((cat) => (
                    <CategoryDropTarget key={cat.id} droppableId={`cat-${cat.id}`} label={cat.name}
                      isOver={draggingOver === cat.id}
                      icon={<FolderOpen className="w-3.5 h-3.5" />} />
                  ))}
                </div>
              </div>
            )}

            {/* List */}
            <div className="flex-1 overflow-y-auto overflow-x-hidden">
              {loading && (
                <div className={cn(
                  'p-4',
                  view === 'grid' ? 'grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3' : 'flex flex-col gap-2.5',
                )}>
                  {Array.from({ length: 9 }).map((_, i) => (
                    <div key={i} className={cn(
                      'rounded-xl border border-border/50 bg-card',
                      view === 'grid' ? 'flex flex-col overflow-hidden' : 'flex flex-row items-start gap-4 p-3',
                    )}>
                      {view === 'grid'
                        ? (<>
                          <Skeleton className="w-full h-36" />
                          <div className="p-3 space-y-2">
                            <Skeleton className="h-4 w-3/4" />
                            <Skeleton className="h-3 w-full" />
                            <Skeleton className="h-3 w-2/3" />
                          </div>
                        </>)
                        : (<>
                          <Skeleton className="w-10 h-10 rounded-lg shrink-0" />
                          <div className="flex-1 space-y-2">
                            <Skeleton className="h-4 w-2/3" />
                            <Skeleton className="h-3 w-full" />
                            <Skeleton className="h-3 w-1/3" />
                          </div>
                        </>)
                      }
                    </div>
                  ))}
                </div>
              )}

              {!loading && items.length === 0 && (
                <div className="flex flex-col items-center justify-center h-64 gap-3 text-muted-foreground">
                  <Inbox className="w-10 h-10 opacity-20" />
                  <p className="text-sm">No items here</p>
                  <p className="text-xs opacity-60">Drop items from other categories or save a new link</p>
                </div>
              )}

              {!loading && (
                <div className={cn(
                  'p-4',
                  view === 'grid' ? 'grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3' : 'flex flex-col gap-2.5',
                )}>
                  {items.map((item) => (
                    <ItemRow
                      key={item.id}
                      item={item}
                      view={view}
                      isDragging={activeItemId === item.id}
                      categories={categories}
                      onOpen={setOpenItem}
                      onAssign={onAssignCategory}
                      onReclassify={onReclassify}
                      onDelete={onDelete}
                    />
                  ))}
                </div>
              )}

              {moreCursor && !loadingMore && (
                <div className="flex justify-center py-5">
                  <Button variant="outline" size="sm" onClick={loadMore}>Load more</Button>
                </div>
              )}
              {loadingMore && (
                <div className="flex justify-center py-5">
                  <RefreshCw className="w-4 h-4 animate-spin text-muted-foreground" />
                </div>
              )}
            </div>
          </div>

      {/* Drag overlay - mirrors list-mode card */}
      <DragOverlay modifiers={[snapCenterToCursor]} style={{ opacity: 0.85 }}>
        {activeItem && (
          <div className={cn(
            'flex flex-row items-start gap-4 p-3 rounded-xl border border-primary/50 bg-card',
            'shadow-2xl shadow-black/40 scale-[1.02] cursor-grabbing ring-1 ring-primary/20',
            'w-[calc(100vw-2rem)] max-w-lg',
          )}>
            <div className="shrink-0 w-10 h-10 rounded-lg bg-muted/50 border border-border/40 overflow-hidden mt-0.5">
              {activeItem.og_image_url ? (
                <img src={activeItem.og_image_url} alt="" className="w-full h-full object-cover" />
              ) : (
                <div className="w-full h-full flex items-center justify-center">
                  <Inbox className="w-4 h-4 text-muted-foreground/30" />
                </div>
              )}
            </div>
            <div className="flex flex-col min-w-0 flex-1 gap-1">
              <p className="font-semibold text-sm leading-snug truncate">
                {activeItem.title || activeItem.url}
              </p>
              {activeItem.summary && (
                <p className="text-xs text-muted-foreground line-clamp-1">{activeItem.summary}</p>
              )}
            </div>
          </div>
        )}
      </DragOverlay>

      {/* Detail sheet */}
      {openItem && (
        <ItemSheet
          item={openItem}
          categories={categories}
          onClose={() => setOpenItem(null)}
          onAssign={(catId) => void onAssignCategory(openItem.id, catId)}
          onReclassify={() => void onReclassify(openItem.id)}
          onDelete={() => void onDelete(openItem.id)}
        />
      )}
    </DndContext>
  )
}
