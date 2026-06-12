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
import { CSS } from '@dnd-kit/utilities'
import {
  Check,
  ExternalLink,
  FolderOpen,
  FolderPlus,
  GitFork,
  Inbox,
  Loader2,
  MoreHorizontal,
  Pencil,
  RefreshCw,
  Star,
  Trash2,
  X,
} from 'lucide-react'
import { useRef, useState } from 'react'

import {
  Badge,
  Button,
  Card,
  CardContent,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  IconButton,
  Input,
  SkeletonList,
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@ui'
import { EmptyState } from '@app'

import type { InboxGhFolder, InboxGhStar } from '@inbox/types'

import { type FolderSelection, useStarsPage } from './useStarsPage'

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

export default function StarsPage() {
  const page = useStarsPage()
  const [activeStar, setActiveStar] = useState<InboxGhStar | null>(null)

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 8 } }),
  )

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
    <DndContext sensors={sensors} onDragStart={handleDragStart} onDragEnd={(e) => void handleDragEnd(e)}>
      <div className="flex h-full min-h-0">
        <FolderSidebar
          folders={page.folders}
          selected={page.selectedFolder}
          onSelect={page.setSelectedFolder}
          onCreateFolder={page.onCreateFolder}
          onRenameFolder={page.onRenameFolder}
          onDeleteFolder={page.onDeleteFolder}
        />

        <div className="flex min-w-0 flex-1 flex-col gap-4 overflow-y-auto p-4 sm:p-6">
          {/* header */}
          <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
            <div>
              <h1 className="text-xl font-semibold">
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
            <div className="flex flex-wrap items-center gap-2">
              <Input
                placeholder="Search..."
                value={page.search}
                onChange={(e) => page.setSearch(e.target.value)}
                className="h-8 w-full sm:w-52"
              />
              <Input
                placeholder="Language"
                value={page.language}
                onChange={(e) => page.setLanguage(e.target.value)}
                className="h-8 w-28"
              />
              <IconButton tooltip="Sync now" variant="outline" onClick={() => void page.onSync()}>
                <RefreshCw className="h-4 w-4" />
              </IconButton>
            </div>
          </div>

          {/* grid */}
          {page.loading ? (
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3">
              <SkeletonList count={6}>{() => <div className="h-44 rounded-lg bg-muted/40" />}</SkeletonList>
            </div>
          ) : page.stars.length === 0 ? (
            <EmptyState message="No stars found" />
          ) : (
            <>
              <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3">
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
      </div>

      {/* drag overlay - ghost card while dragging */}
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

interface FolderSidebarProps {
  folders: InboxGhFolder[]
  selected: FolderSelection
  onSelect: (f: FolderSelection) => void
  onCreateFolder: (body: { name: string }) => Promise<InboxGhFolder | null>
  onRenameFolder: (id: number, body: { name: string }) => Promise<void>
  onDeleteFolder: (id: number) => Promise<void>
}

function FolderSidebar({
  folders,
  selected,
  onSelect,
  onCreateFolder,
  onRenameFolder,
  onDeleteFolder,
}: FolderSidebarProps) {
  const [creating, setCreating] = useState(false)
  const [newName, setNewName] = useState('')
  const [renamingId, setRenamingId] = useState<number | null>(null)
  const [renameValue, setRenameValue] = useState('')
  const createRef = useRef<HTMLInputElement>(null)
  const renameRef = useRef<HTMLInputElement>(null)

  function startCreate() {
    setCreating(true)
    setNewName('')
    setTimeout(() => createRef.current?.focus(), 0)
  }

  async function confirmCreate() {
    const name = newName.trim()
    setCreating(false)
    setNewName('')
    if (!name) return
    const f = await onCreateFolder({ name })
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

  return (
    <aside className="flex w-12 shrink-0 flex-col gap-0.5 overflow-y-auto border-r border-border bg-muted/20 py-3 sm:w-52 sm:px-2">
      <SidebarEntry id="all" label="All stars" selected={selected} onSelect={onSelect}>
        <Star className="h-4 w-4 shrink-0" />
      </SidebarEntry>
      <SidebarEntry id="unorganised" label="Unorganised" selected={selected} onSelect={onSelect}>
        <Inbox className="h-4 w-4 shrink-0" />
      </SidebarEntry>

      {folders.length > 0 && (
        <div className="mx-2 mb-1 mt-3 hidden border-t border-border sm:block" />
      )}
      {folders.length > 0 && (
        <p className="hidden px-2 pb-0.5 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground/60 sm:block">
          Folders
        </p>
      )}

      {folders.map((folder) =>
        renamingId === folder.id ? (
          <div key={folder.id} className="mx-1 hidden items-center gap-1 sm:flex">
            <input
              ref={renameRef}
              className="h-7 flex-1 rounded border border-border bg-background px-2 text-xs outline-none focus:ring-1 focus:ring-primary"
              value={renameValue}
              onChange={(e) => setRenameValue(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void confirmRename()
                if (e.key === 'Escape') setRenamingId(null)
              }}
              onBlur={() => void confirmRename()}
            />
            <button type="button" className="p-0.5 text-muted-foreground hover:text-foreground" onClick={() => void confirmRename()}>
              <Check className="h-3.5 w-3.5" />
            </button>
          </div>
        ) : (
          <div key={folder.id} className="group relative">
            <SidebarEntry id={folder.id} label={folder.name} selected={selected} onSelect={onSelect} count={folder.star_count}>
              <FolderOpen className="h-4 w-4 shrink-0" />
            </SidebarEntry>
            {/* folder actions - desktop only */}
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  className="absolute right-1 top-1/2 hidden -translate-y-1/2 items-center justify-center rounded p-0.5 opacity-0 hover:bg-muted group-hover:opacity-100 sm:flex"
                >
                  <MoreHorizontal className="h-3.5 w-3.5 text-muted-foreground" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-36">
                <DropdownMenuItem onClick={() => startRename(folder)}>
                  <Pencil className="mr-2 h-3.5 w-3.5" />Rename
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  className="text-destructive focus:text-destructive"
                  onClick={() => void onDeleteFolder(folder.id)}
                >
                  <Trash2 className="mr-2 h-3.5 w-3.5" />Delete
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        )
      )}

      {/* new folder */}
      {creating ? (
        <div className="mx-1 mt-1 hidden items-center gap-1 sm:flex">
          <input
            ref={createRef}
            className="h-7 flex-1 rounded border border-border bg-background px-2 text-xs outline-none focus:ring-1 focus:ring-primary"
            placeholder="Folder name"
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void confirmCreate()
              if (e.key === 'Escape') setCreating(false)
            }}
            onBlur={() => void confirmCreate()}
          />
          <button type="button" className="p-0.5 text-muted-foreground hover:text-foreground" onClick={() => setCreating(false)}>
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
      ) : (
        <button
          type="button"
          onClick={startCreate}
          className="mt-1 flex items-center gap-2 rounded-md px-2 py-1.5 text-xs text-muted-foreground/50 transition-colors hover:bg-muted hover:text-muted-foreground"
        >
          <FolderPlus className="h-4 w-4 shrink-0" />
          <span className="hidden sm:block">New folder</span>
        </button>
      )}
    </aside>
  )
}

// ---------------------------------------------------------------------------
// Droppable sidebar entry
// ---------------------------------------------------------------------------

interface SidebarEntryProps {
  id: FolderSelection
  label: string
  selected: FolderSelection
  count?: number | null
  onSelect: (f: FolderSelection) => void
  children: React.ReactNode
}

function SidebarEntry({ id, label, selected, count, onSelect, children }: SidebarEntryProps) {
  const { isOver, setNodeRef } = useDroppable({ id: id as string | number })
  const active = selected === id

  return (
    <button
      ref={setNodeRef}
      type="button"
      onClick={() => onSelect(id)}
      className={[
        'flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-sm transition-colors',
        active
          ? 'bg-primary/15 font-medium text-primary'
          : 'text-muted-foreground hover:bg-muted hover:text-foreground',
        isOver ? 'ring-2 ring-primary ring-inset' : '',
      ].join(' ')}
    >
      {children}
      <span className="hidden flex-1 truncate text-left sm:block">{label}</span>
      {count != null && (
        <span className="hidden shrink-0 text-[10px] text-muted-foreground sm:block">{count}</span>
      )}
    </button>
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
  const { attributes, listeners, setNodeRef, transform } = useDraggable({ id: star.id })

  const style = transform
    ? { transform: CSS.Translate.toString(transform) }
    : undefined

  return (
    <div
      ref={setNodeRef}
      style={style}
      className={isDragging ? 'opacity-30' : undefined}
    >
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
// Star card content (shared between draggable and overlay)
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

  return (
    <Card className={['flex flex-col gap-0 select-none', ghost ? 'pointer-events-none' : ''].join(' ')}>
      {/* drag handle + header */}
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
          <span className="truncate text-sm font-semibold leading-snug">
            {star.full_name}
          </span>
          {star.language && (
            <span className="text-xs text-muted-foreground">{star.language}</span>
          )}
        </div>
        {/* external link - stops drag propagation so clicking still works */}
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

      <CardContent className="flex flex-wrap items-center gap-1.5 px-4 pb-3 pt-0">
        {star.stargazers_count != null && (
          <span className="flex items-center gap-0.5 text-xs text-muted-foreground">
            <Star className="h-3 w-3" />
            {star.stargazers_count.toLocaleString()}
          </span>
        )}
        {star.fork && (
          <span className="flex items-center gap-0.5 text-xs text-muted-foreground">
            <GitFork className="h-3 w-3" />
            Fork
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
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="-mr-1 h-6 w-6"
                    onPointerDown={(e: React.PointerEvent) => e.stopPropagation()}
                  >
                    <MoreHorizontal className="h-3.5 w-3.5" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>Actions</TooltipContent>
              </Tooltip>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-48">
              {folders
                .filter((f) => f.id !== (typeof selectedFolder === 'number' ? selectedFolder : -1))
                .map((f) => (
                  <DropdownMenuItem key={f.id} onClick={() => onMoveTo?.(f.id)}>
                    <FolderOpen className="mr-2 h-3.5 w-3.5" />
                    Move to {f.name}
                  </DropdownMenuItem>
                ))}
              {typeof selectedFolder === 'number' && (
                <DropdownMenuItem onClick={() => onMoveTo?.('all')}>
                  <X className="mr-2 h-3.5 w-3.5" />
                  Remove from folder
                </DropdownMenuItem>
              )}
              {(folders.length > 0 || typeof selectedFolder === 'number') && star.can_unstar && (
                <DropdownMenuSeparator />
              )}
              {star.can_unstar && (
                <DropdownMenuItem
                  className="text-destructive focus:text-destructive"
                  onPointerDown={(e: React.PointerEvent) => e.stopPropagation()}
                  onClick={onUnstar}
                >
                  <Star className="mr-2 h-3.5 w-3.5" />
                  Unstar on GitHub
                </DropdownMenuItem>
              )}
            </DropdownMenuContent>
          </DropdownMenu>
        )}
      </CardContent>
    </Card>
  )
}
