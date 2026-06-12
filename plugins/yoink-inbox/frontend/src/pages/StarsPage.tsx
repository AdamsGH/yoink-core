import {
  DndContext,
  DragOverlay,
  PointerSensor,
  useDraggable,
  useDroppable,
  useSensor,
  useSensors,
  type DragEndEvent,
  type DragStartEvent,
} from '@dnd-kit/core'
import {
  SortableContext,
  arrayMove,
  useSortable,
  verticalListSortingStrategy,
} from '@dnd-kit/sortable'
import { CSS } from '@dnd-kit/utilities'
import {
  ChevronRight,
  ExternalLink,
  FolderOpen,
  FolderPlus,
  GitFork,
  GripVertical,
  Inbox,
  Loader2,
  MoreHorizontal,
  Pencil,
  Pin,
  PinOff,
  RefreshCw,
  Star,
  Trash2,
  X,
} from 'lucide-react'
import { useEffect, useRef, useState } from 'react'

import { useRightSidebar } from '@core/layout/AppLayout'
import {
  Badge,
  Button,
  Card,
  CardContent,
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  Input,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  SidebarGroup,
  SidebarGroupAction,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuAction,
  SidebarMenuButton,
  SidebarMenuItem,
  SkeletonList,
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@ui'
import { EmptyState } from '@app'

import type { InboxGhFolder, InboxGhStar } from '@inbox/types'

import type { GhStarSort } from '@inbox/api/items'
import { type FolderSelection, useStarsPage } from './useStarsPage'

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

export default function StarsPage() {
  const page = useStarsPage()
  const [activeStar, setActiveStar] = useState<InboxGhStar | null>(null)
  const { setContent } = useRightSidebar()

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 8 } }),
  )

  useEffect(() => {
    setContent(
      <FolderSidebarContent
        folders={page.folders}
        selected={page.selectedFolder}
        onSelect={page.setSelectedFolder}
        onCreateFolder={page.onCreateFolder}
        onRenameFolder={page.onRenameFolder}
        onDeleteFolder={page.onDeleteFolder}
        onPinFolder={page.onPinFolder}
        onReorderPinned={page.onReorderPinned}
        onDropOnFolder={async (folder) => {
          if (activeStar !== null) {
            await page.onMoveStar(activeStar.id, folder)
          }
        }}
      />,
    )
    return () => setContent(null)
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [setContent, page.folders, page.selectedFolder, activeStar])

  function handleDragStart(event: DragStartEvent) {
    const star = page.stars.find((s) => s.id === event.active.id)
    setActiveStar(star ?? null)
  }

  async function handleDragEnd(event: DragEndEvent) {
    setActiveStar(null)
    const { active, over } = event
    if (!over) return
    const starId = active.id as number
    const target = over.id as FolderSelection
    if (target === page.selectedFolder) return
    await page.onMoveStar(starId, target)
  }

  return (
    <DndContext
      sensors={sensors}
      onDragStart={handleDragStart}
      onDragEnd={(e) => void handleDragEnd(e)}
    >
      <div className="flex h-full min-h-0 flex-col gap-4 overflow-y-auto p-4 sm:p-6">
        {/* header */}
        <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
          <div className="min-w-0">
            <h1 className="truncate text-xl font-semibold">
              {page.selectedFolder === 'all'
                ? 'GitHub Stars'
                : page.selectedFolder === 'unorganised'
                  ? 'Unorganised'
                  : (page.folders.find((f) => f.id === page.selectedFolder)?.name ?? 'Folder')}
            </h1>
            {page.syncStatus && (
              <p className="text-xs text-muted-foreground">
                Last sync: {page.syncStatus}
                {page.lastSync ? ` (${new Date(page.lastSync).toLocaleString()})` : ''}
              </p>
            )}
          </div>
          <div className="flex shrink-0 flex-wrap items-center gap-2">
            <Input
              placeholder="Search..."
              value={page.search}
              onChange={(e) => page.setSearch(e.target.value)}
              className="h-8 w-44"
            />
            <Select
              value={page.sort}
              onValueChange={(v) => page.setSort(v as GhStarSort)}
            >
              <SelectTrigger className="h-8 w-40">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="starred_at">Recently starred</SelectItem>
                <SelectItem value="stargazers_count">Most stars</SelectItem>
                <SelectItem value="name">Name</SelectItem>
              </SelectContent>
            </Select>
            <Select value={page.language} onValueChange={page.setLanguage}>
              <SelectTrigger className="h-8 w-36">
                <SelectValue placeholder="All languages" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="">All languages</SelectItem>
                {page.languages.map((l) => (
                  <SelectItem key={l} value={l}>{l}</SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="outline"
                  size="icon"
                  className="h-8 w-8 shrink-0"
                  onClick={() => void page.onSync()}
                >
                  <RefreshCw className="h-4 w-4" />
                </Button>
              </TooltipTrigger>
              <TooltipContent>Sync now</TooltipContent>
            </Tooltip>
          </div>
        </div>

        {/* grid */}
        {page.loading ? (
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3 2xl:grid-cols-4">
            <SkeletonList count={8}>{() => <div className="h-44 rounded-lg bg-muted/40" />}</SkeletonList>
          </div>
        ) : page.stars.length === 0 ? (
          <EmptyState message="No stars found" />
        ) : (
          <>
            <div className="grid grid-cols-1 items-start gap-3 sm:grid-cols-2 lg:grid-cols-3 2xl:grid-cols-4">
              {page.stars.map((star) => (
                <DraggableStarCard
                  key={star.id}
                  star={star}
                  folders={page.folders}
                  selectedFolder={page.selectedFolder}
                  moving={page.movingId === star.id}
                  isDragging={activeStar?.id === star.id}
                  onMoveTo={(fid) => void page.onMoveStar(star.id, fid)}
                  onUnstar={() => void page.onUnstar(star.id, star.gh_repo_id)}
                />
              ))}
            </div>
            {page.moreCursor && (
              <div className="flex justify-center pt-2">
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => void page.loadMore()}
                  disabled={page.loadingMore}
                >
                  {page.loadingMore
                    ? <><Loader2 className="mr-2 h-3.5 w-3.5 animate-spin" />Loading...</>
                    : 'Load more'}
                </Button>
              </div>
            )}
          </>
        )}
      </div>

      <DragOverlay dropAnimation={null}>
        {activeStar && (
          <div className="w-72 rotate-2 opacity-90 shadow-2xl">
            <StarCardContent star={activeStar} ghost />
          </div>
        )}
      </DragOverlay>
    </DndContext>
  )
}

// ---------------------------------------------------------------------------
// Folder sidebar
// ---------------------------------------------------------------------------

interface FolderSidebarContentProps {
  folders: InboxGhFolder[]
  selected: FolderSelection
  onSelect: (f: FolderSelection) => void
  onCreateFolder: (body: { name: string; is_local?: boolean }) => Promise<InboxGhFolder | null>
  onRenameFolder: (id: number, body: { name: string }) => Promise<void>
  onDeleteFolder: (id: number) => Promise<void>
  onPinFolder: (id: number, pinned: boolean) => Promise<void>
  onReorderPinned: (orderedIds: number[]) => Promise<void>
  onDropOnFolder: (f: FolderSelection) => Promise<void>
}

function FolderSidebarContent({
  folders,
  selected,
  onSelect,
  onCreateFolder,
  onRenameFolder,
  onDeleteFolder,
  onPinFolder,
  onReorderPinned,
  onDropOnFolder,
}: FolderSidebarContentProps) {
  const [creating, setCreating] = useState(false)
  const [createIsLocal, setCreateIsLocal] = useState(true)
  const [newName, setNewName] = useState('')
  const [renamingId, setRenamingId] = useState<number | null>(null)
  const [renameValue, setRenameValue] = useState('')
  const createRef = useRef<HTMLInputElement>(null)
  const renameRef = useRef<HTMLInputElement>(null)

  const ghListsCollapsedKey = 'stars-sidebar-gh-lists-collapsed'
  const [ghListsCollapsed, setGhListsCollapsed] = useState<boolean>(() => {
    try { return localStorage.getItem(ghListsCollapsedKey) !== 'false' }
    catch { return true }
  })

  function toggleGhLists(open: boolean) {
    setGhListsCollapsed(!open)
    try { localStorage.setItem(ghListsCollapsedKey, String(!open)) } catch { /* ignore */ }
  }

  const pinned = [...folders.filter((f) => f.is_pinned)].sort((a, b) => a.sort_order - b.sort_order)
  const local = folders.filter((f) => !f.is_pinned && f.gh_list_id === null).sort((a, b) => a.name.localeCompare(b.name))
  const ghLists = folders.filter((f) => f.gh_list_id !== null).sort((a, b) => a.name.localeCompare(b.name))

  // Sortable DnD for pinned folders (separate context from star card DnD)
  const pinnedSensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
  )

  function handlePinnedDragEnd(event: DragEndEvent) {
    const { active, over } = event
    if (!over || active.id === over.id) return
    const oldIdx = pinned.findIndex((f) => f.id === active.id)
    const newIdx = pinned.findIndex((f) => f.id === over.id)
    if (oldIdx < 0 || newIdx < 0) return
    const reordered = arrayMove(pinned, oldIdx, newIdx)
    void onReorderPinned(reordered.map((f) => f.id))
  }

  function startCreate(isLocal: boolean) {
    setCreateIsLocal(isLocal)
    setCreating(true)
    setNewName('')
    setTimeout(() => createRef.current?.focus(), 0)
  }

  async function confirmCreate() {
    const name = newName.trim()
    setCreating(false)
    setNewName('')
    if (!name) return
    const f = await onCreateFolder({ name, is_local: createIsLocal })
    if (f) onSelect(f.id)
  }

  function startRename(folder: InboxGhFolder) {
    setRenamingId(folder.id)
    setRenameValue(folder.name)
    setTimeout(() => renameRef.current?.focus(), 0)
  }

  async function confirmRename() {
    if (!renamingId) return
    const name = renameValue.trim()
    setRenamingId(null)
    if (name) await onRenameFolder(renamingId, { name })
  }

  function renderInlineEdit(ref: React.RefObject<HTMLInputElement | null>, value: string, onChange: (v: string) => void, onConfirm: () => void, onCancel: () => void) {
    return (
      <div className="flex items-center gap-1 px-2">
        <input
          ref={ref}
          className="h-7 flex-1 rounded border border-border bg-background px-2 text-xs outline-none focus:ring-1 focus:ring-primary"
          placeholder="Folder name"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') onConfirm()
            if (e.key === 'Escape') onCancel()
          }}
          onBlur={onConfirm}
        />
        <button type="button" className="p-0.5 text-muted-foreground hover:text-foreground" onClick={onCancel}>
          <X className="h-3.5 w-3.5" />
        </button>
      </div>
    )
  }

  function renderFolderItem(folder: InboxGhFolder, actions: React.ReactNode, extraProps?: { sortable?: boolean }) {
    const isRenaming = renamingId === folder.id
    if (isRenaming) {
      return (
        <SidebarMenuItem key={folder.id}>
          {renderInlineEdit(renameRef, renameValue, setRenameValue, () => void confirmRename(), () => setRenamingId(null))}
        </SidebarMenuItem>
      )
    }
    return (
      <SidebarMenuItem key={folder.id} className="group/folder-item">
        <DroppableMenuItem
          id={folder.id}
          isActive={selected === folder.id}
          onSelect={() => onSelect(folder.id)}
          onDrop={() => onDropOnFolder(folder.id)}
          icon={folder.gh_list_id
            ? <GitFork className="h-4 w-4 text-muted-foreground/60" />
            : <FolderOpen className="h-4 w-4" />}
          label={folder.name}
          badge={folder.star_count ?? undefined}
          sortable={extraProps?.sortable}
          folderId={folder.id}
        />
        {actions}
      </SidebarMenuItem>
    )
  }

  function folderActionMenu(folder: InboxGhFolder, opts: { canPin?: boolean; canUnpin?: boolean; canDelete?: boolean }) {
    return (
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <SidebarMenuAction showOnHover className="peer-data-[active=true]/menu-button:opacity-100">
            <MoreHorizontal className="h-3.5 w-3.5" />
            <span className="sr-only">Folder actions</span>
          </SidebarMenuAction>
        </DropdownMenuTrigger>
        <DropdownMenuContent side="right" align="start" className="w-40">
          <DropdownMenuItem onClick={() => startRename(folder)}>
            <Pencil className="mr-2 h-3.5 w-3.5" />Rename
          </DropdownMenuItem>
          {opts.canPin && (
            <DropdownMenuItem onClick={() => void onPinFolder(folder.id, true)}>
              <Pin className="mr-2 h-3.5 w-3.5" />Pin
            </DropdownMenuItem>
          )}
          {opts.canUnpin && (
            <DropdownMenuItem onClick={() => void onPinFolder(folder.id, false)}>
              <PinOff className="mr-2 h-3.5 w-3.5" />Unpin
            </DropdownMenuItem>
          )}
          {opts.canDelete && (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                className="text-destructive focus:text-destructive"
                onClick={() => void onDeleteFolder(folder.id)}
              >
                <Trash2 className="mr-2 h-3.5 w-3.5" />Delete
              </DropdownMenuItem>
            </>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
    )
  }

  return (
    <>
      {/* All / Unorganised */}
      <SidebarGroup>
        <SidebarGroupContent>
          <SidebarMenu>
            <DroppableMenuItem
              id="all"
              isActive={selected === 'all'}
              onSelect={() => onSelect('all')}
              onDrop={() => onDropOnFolder('all')}
              icon={<Star className="h-4 w-4" />}
              label="All stars"
            />
            <DroppableMenuItem
              id="unorganised"
              isActive={selected === 'unorganised'}
              onSelect={() => onSelect('unorganised')}
              onDrop={() => onDropOnFolder('unorganised')}
              icon={<Inbox className="h-4 w-4" />}
              label="Unorganised"
            />
          </SidebarMenu>
        </SidebarGroupContent>
      </SidebarGroup>

      {/* Pinned */}
      {pinned.length > 0 && (
        <SidebarGroup>
          <SidebarGroupLabel>Pinned</SidebarGroupLabel>
          <SidebarGroupContent>
            <SidebarMenu>
              <DndContext sensors={pinnedSensors} onDragEnd={handlePinnedDragEnd}>
                <SortableContext items={pinned.map((f) => f.id)} strategy={verticalListSortingStrategy}>
                  {pinned.map((folder) =>
                    renderFolderItem(
                      folder,
                      folderActionMenu(folder, { canUnpin: true, canDelete: true }),
                      { sortable: true },
                    ),
                  )}
                </SortableContext>
              </DndContext>
            </SidebarMenu>
          </SidebarGroupContent>
        </SidebarGroup>
      )}

      {/* My Folders */}
      <SidebarGroup>
        <SidebarGroupLabel>My Folders</SidebarGroupLabel>
        {!creating ? (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <SidebarGroupAction title="New folder">
                <FolderPlus className="h-4 w-4" />
                <span className="sr-only">New folder</span>
              </SidebarGroupAction>
            </DropdownMenuTrigger>
            <DropdownMenuContent side="right" align="start" className="w-44">
              <DropdownMenuItem onClick={() => startCreate(true)}>
                <FolderOpen className="mr-2 h-3.5 w-3.5" />Local folder
              </DropdownMenuItem>
              <DropdownMenuItem onClick={() => startCreate(false)}>
                <GitFork className="mr-2 h-3.5 w-3.5" />GitHub List
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        ) : null}
        <SidebarGroupContent>
          <SidebarMenu>
            {creating && (
              <SidebarMenuItem>
                <div className="px-2 pb-1">
                  <p className="mb-1 text-[10px] text-muted-foreground">
                    {createIsLocal ? 'Local folder' : 'GitHub List (synced)'}
                  </p>
                  {renderInlineEdit(createRef, newName, setNewName, () => void confirmCreate(), () => setCreating(false))}
                </div>
              </SidebarMenuItem>
            )}
            {local.map((folder) =>
              renderFolderItem(
                folder,
                folderActionMenu(folder, { canPin: true, canDelete: true }),
              ),
            )}
          </SidebarMenu>
        </SidebarGroupContent>
      </SidebarGroup>

      {/* GitHub Lists (collapsible) */}
      {ghLists.length > 0 && (
        <SidebarGroup>
          <Collapsible open={!ghListsCollapsed} onOpenChange={toggleGhLists}>
            <CollapsibleTrigger asChild>
              <SidebarGroupLabel className="cursor-pointer select-none">
                <span className="flex flex-1 items-center gap-1">
                  <ChevronRight
                    className={[
                      'h-3.5 w-3.5 transition-transform duration-200',
                      ghListsCollapsed ? '' : 'rotate-90',
                    ].join(' ')}
                  />
                  GitHub Lists
                  <span className="ml-auto text-[10px] tabular-nums text-muted-foreground/60">
                    {ghLists.length}
                  </span>
                </span>
              </SidebarGroupLabel>
            </CollapsibleTrigger>
            <CollapsibleContent>
              <SidebarGroupContent>
                <SidebarMenu>
                  {ghLists.map((folder) =>
                    renderFolderItem(
                      folder,
                      folderActionMenu(folder, { canPin: true }),
                    ),
                  )}
                </SidebarMenu>
              </SidebarGroupContent>
            </CollapsibleContent>
          </Collapsible>
        </SidebarGroup>
      )}
    </>
  )
}

// ---------------------------------------------------------------------------
// Droppable + optionally sortable menu item
// ---------------------------------------------------------------------------

function DroppableMenuItem({
  id,
  isActive,
  onSelect,
  onDrop,
  icon,
  label,
  badge,
  sortable,
  folderId,
}: {
  id: FolderSelection
  isActive: boolean
  onSelect: () => void
  onDrop: () => void
  icon: React.ReactNode
  label: string
  badge?: number
  sortable?: boolean
  folderId?: number
}) {
  const { isOver, setNodeRef: setDropRef } = useDroppable({ id: id as string | number })
  const sortable_ = useSortable({ id: folderId ?? 0, disabled: !sortable })

  const setRef = (el: HTMLElement | null) => {
    setDropRef(el)
    if (sortable) sortable_.setNodeRef(el)
  }

  const style = sortable
    ? { transform: CSS.Transform.toString(sortable_.transform), transition: sortable_.transition }
    : undefined

  return (
    <SidebarMenuItem style={style}>
      {sortable && (
        <SidebarMenuAction
          className="left-0 cursor-grab text-muted-foreground/40 hover:text-muted-foreground active:cursor-grabbing"
          {...sortable_.attributes}
          {...sortable_.listeners}
        >
          <GripVertical className="h-3.5 w-3.5" />
        </SidebarMenuAction>
      )}
      <SidebarMenuButton
        ref={setRef as React.Ref<HTMLButtonElement>}
        isActive={isActive}
        onClick={onSelect}
        className={[
          'pr-8',
          sortable ? 'pl-7' : '',
          isOver
            ? 'bg-primary/15 ring-2 ring-primary/60 ring-inset transition-colors duration-150'
            : 'transition-colors duration-150',
        ].join(' ')}
        // eslint-disable-next-line @typescript-eslint/no-misused-promises
        onDrop={async (e) => { e.preventDefault(); onDrop() }}
        onDragOver={(e) => e.preventDefault()}
      >
        {icon}
        <span className="flex-1 truncate">{label}</span>
        {badge != null && (
          <span className="ml-auto shrink-0 tabular-nums text-[10px] text-sidebar-foreground/60 transition-opacity group-hover/menu-item:opacity-0">
            {badge}
          </span>
        )}
      </SidebarMenuButton>
    </SidebarMenuItem>
  )
}

// ---------------------------------------------------------------------------
// Draggable star card
// ---------------------------------------------------------------------------

interface DraggableStarCardProps {
  star: InboxGhStar
  folders: InboxGhFolder[]
  selectedFolder: FolderSelection
  moving: boolean
  isDragging: boolean
  onMoveTo: (folder: FolderSelection) => void
  onUnstar: () => void
}

function DraggableStarCard({
  star,
  folders,
  selectedFolder,
  moving,
  isDragging,
  onMoveTo,
  onUnstar,
}: DraggableStarCardProps) {
  const { attributes, listeners, setNodeRef } = useDraggable({ id: star.id })

  return (
    <div ref={setNodeRef} className={isDragging ? 'opacity-0' : undefined}>
      <StarCardContent
        star={star}
        folders={folders}
        selectedFolder={selectedFolder}
        moving={moving}
        dragHandleProps={{ ...attributes, ...listeners }}
        onMoveTo={onMoveTo}
        onUnstar={onUnstar}
      />
    </div>
  )
}

// ---------------------------------------------------------------------------
// Star card content
// ---------------------------------------------------------------------------

interface StarCardContentProps {
  star: InboxGhStar
  folders?: InboxGhFolder[]
  selectedFolder?: FolderSelection
  moving?: boolean
  ghost?: boolean
  dragHandleProps?: React.HTMLAttributes<HTMLElement>
  onMoveTo?: (folder: FolderSelection) => void
  onUnstar?: () => void
}

function StarCardContent({
  star,
  folders = [],
  selectedFolder = 'all',
  moving = false,
  ghost = false,
  dragHandleProps,
  onMoveTo,
  onUnstar,
}: StarCardContentProps) {
  const repoUrl = `https://github.com/${star.full_name}`

  const folderIds = star.folder_ids ?? []
  const memberFolders = folderIds
    .map((fid) => folders.find((f) => f.id === fid))
    .filter((f): f is InboxGhFolder => f !== undefined)

  // Group folders for move-to menu
  const pinnedFolders = folders.filter((f) => f.is_pinned)
  const localFolders = folders.filter((f) => !f.is_pinned && f.gh_list_id === null)
  const ghListFolders = folders.filter((f) => f.gh_list_id !== null)
  const currentFolderId = typeof selectedFolder === 'number' ? selectedFolder : -1

  return (
    <Card className={['select-none', ghost ? 'pointer-events-none' : ''].join(' ')}>
      <div
        className="flex cursor-grab items-start gap-3 px-4 pb-2 pt-3 active:cursor-grabbing"
        {...dragHandleProps}
      >
        {star.owner_avatar_url ? (
          <img src={star.owner_avatar_url} alt={star.owner_login ?? ''} className="h-7 w-7 shrink-0 rounded-full" />
        ) : (
          <div className="h-7 w-7 shrink-0 rounded-full bg-muted" />
        )}
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="truncate text-sm font-semibold leading-snug">{star.full_name}</span>
          {star.language && <span className="text-xs text-muted-foreground">{star.language}</span>}
        </div>
        <a
          href={repoUrl}
          target="_blank"
          rel="noopener noreferrer"
          className="shrink-0 text-muted-foreground hover:text-foreground"
          onPointerDown={(e) => e.stopPropagation()}
        >
          <ExternalLink className="h-3.5 w-3.5" />
        </a>
      </div>

      {star.description && (
        <p className="px-4 pb-2 text-xs leading-relaxed text-muted-foreground line-clamp-3">
          {star.description}
        </p>
      )}

      {/* Folder membership badges */}
      {!ghost && memberFolders.length > 0 && (
        <div className="flex flex-wrap gap-1 px-4 pb-2">
          {memberFolders.slice(0, 3).map((f) => (
            <Badge
              key={f.id}
              variant="outline"
              className="cursor-pointer gap-1 px-1.5 py-0 text-[10px] hover:bg-accent"
              onClick={() => onMoveTo?.(f.id)}
            >
              <FolderOpen className="h-2.5 w-2.5" />
              {f.name}
            </Badge>
          ))}
          {memberFolders.length > 3 && (
            <Badge variant="outline" className="px-1.5 py-0 text-[10px]">
              +{memberFolders.length - 3}
            </Badge>
          )}
        </div>
      )}

      <CardContent className="flex flex-wrap items-center gap-1.5 px-4 pb-3 pt-0">
        {star.stargazers_count != null && (
          <span className="flex items-center gap-0.5 text-xs text-muted-foreground">
            <Star className="h-3 w-3" />
            {star.stargazers_count.toLocaleString()}
          </span>
        )}
        {star.fork && (
          <span className="flex items-center gap-0.5 text-xs text-muted-foreground">
            <GitFork className="h-3 w-3" />Fork
          </span>
        )}
        {star.topics?.slice(0, 4).map((t) => (
          <Badge key={t} variant="secondary" className="px-1.5 py-0 text-[10px]">{t}</Badge>
        ))}

        <div className="flex-1" />

        {moving && <Loader2 className="h-3.5 w-3.5 animate-spin text-muted-foreground" />}

        {!ghost && (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                className="flex h-6 w-6 items-center justify-center rounded hover:bg-muted"
                onPointerDown={(e: React.PointerEvent) => e.stopPropagation()}
              >
                <MoreHorizontal className="h-3.5 w-3.5" />
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-52">
              {pinnedFolders.length > 0 && (
                <>
                  <DropdownMenuLabel className="text-[10px] font-normal text-muted-foreground">Pinned</DropdownMenuLabel>
                  {pinnedFolders
                    .filter((f) => f.id !== currentFolderId)
                    .map((f) => (
                      <DropdownMenuItem key={f.id} onClick={() => onMoveTo?.(f.id)}>
                        <FolderOpen className="mr-2 h-3.5 w-3.5" />{f.name}
                      </DropdownMenuItem>
                    ))}
                </>
              )}
              {localFolders.length > 0 && (
                <>
                  {pinnedFolders.length > 0 && <DropdownMenuSeparator />}
                  <DropdownMenuLabel className="text-[10px] font-normal text-muted-foreground">My Folders</DropdownMenuLabel>
                  {localFolders
                    .filter((f) => f.id !== currentFolderId)
                    .map((f) => (
                      <DropdownMenuItem key={f.id} onClick={() => onMoveTo?.(f.id)}>
                        <FolderOpen className="mr-2 h-3.5 w-3.5" />{f.name}
                      </DropdownMenuItem>
                    ))}
                </>
              )}
              {ghListFolders.length > 0 && (
                <>
                  {(pinnedFolders.length > 0 || localFolders.length > 0) && <DropdownMenuSeparator />}
                  <DropdownMenuLabel className="text-[10px] font-normal text-muted-foreground">GitHub Lists</DropdownMenuLabel>
                  {ghListFolders
                    .filter((f) => f.id !== currentFolderId)
                    .map((f) => (
                      <DropdownMenuItem key={f.id} onClick={() => onMoveTo?.(f.id)}>
                        <GitFork className="mr-2 h-3.5 w-3.5" />{f.name}
                      </DropdownMenuItem>
                    ))}
                </>
              )}
              {typeof selectedFolder === 'number' && (
                <>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem onClick={() => onMoveTo?.('all')}>
                    <X className="mr-2 h-3.5 w-3.5" />Remove from folder
                  </DropdownMenuItem>
                </>
              )}
              {star.can_unstar && (
                <>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem
                    className="text-destructive focus:text-destructive"
                    onPointerDown={(e: React.PointerEvent) => e.stopPropagation()}
                    onClick={onUnstar}
                  >
                    <Star className="mr-2 h-3.5 w-3.5" />Unstar on GitHub
                  </DropdownMenuItem>
                </>
              )}
            </DropdownMenuContent>
          </DropdownMenu>
        )}
      </CardContent>
    </Card>
  )
}
