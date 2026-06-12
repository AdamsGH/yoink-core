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
import { useEffect, useRef, useState } from 'react'

import { useRightSidebar } from '@core/layout/AppLayout'
import {
  Badge,
  Button,
  Card,
  CardContent,
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  Input,
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

  // Mount folder panel in the right sidebar for the lifetime of this page
  useEffect(() => {
    setContent(
      <FolderSidebarContent
        folders={page.folders}
        selected={page.selectedFolder}
        onSelect={page.setSelectedFolder}
        onCreateFolder={page.onCreateFolder}
        onRenameFolder={page.onRenameFolder}
        onDeleteFolder={page.onDeleteFolder}
        onDropOnFolder={async (folder) => {
          if (activeStar !== null) {
            await page.onMoveStar(activeStar.id, folder)
          }
        }}
      />
    )
    return () => setContent(null)
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [setContent, page.folders, page.selectedFolder])

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
          <div className="flex shrink-0 items-center gap-2">
            <Input
              placeholder="Search..."
              value={page.search}
              onChange={(e) => page.setSearch(e.target.value)}
              className="h-8 w-48"
            />
            <Combobox<string>
              items={['', ...page.languages]}
              itemToStringLabel={(l) => l === '' ? 'All languages' : l}
              itemToStringValue={(l) => l}
            >
              <ComboboxInput
                value={page.language === '' ? 'All languages' : page.language}
                placeholder="Language"
                className="h-8 w-36"
              />
              <ComboboxContent>
                <ComboboxEmpty>No languages found</ComboboxEmpty>
                <ComboboxList>
                  {(lang) => (
                    <ComboboxItem
                      key={lang}
                      value={lang}
                      onSelect={() => page.setLanguage(lang)}
                    >
                      {lang === '' ? 'All languages' : lang}
                    </ComboboxItem>
                  )}
                </ComboboxList>
              </ComboboxContent>
            </Combobox>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="outline" size="icon" className="h-8 w-8 shrink-0" onClick={() => void page.onSync()}>
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
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3 2xl:grid-cols-4 items-start">
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
// Folder sidebar content - rendered inside the app-level right Sidebar
// ---------------------------------------------------------------------------

interface FolderSidebarContentProps {
  folders: InboxGhFolder[]
  selected: FolderSelection
  onSelect: (f: FolderSelection) => void
  onCreateFolder: (body: { name: string }) => Promise<InboxGhFolder | null>
  onRenameFolder: (id: number, body: { name: string }) => Promise<void>
  onDeleteFolder: (id: number) => Promise<void>
  onDropOnFolder: (f: FolderSelection) => Promise<void>
}

function FolderSidebarContent({
  folders,
  selected,
  onSelect,
  onCreateFolder,
  onRenameFolder,
  onDeleteFolder,
  onDropOnFolder,
}: FolderSidebarContentProps) {
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
    <>
      {/* Static entries */}
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

      {/* Folders group */}
      <SidebarGroup>
        <SidebarGroupLabel>Folders</SidebarGroupLabel>
        <SidebarGroupAction title="New folder" onClick={startCreate}>
          <FolderPlus className="h-4 w-4" />
          <span className="sr-only">New folder</span>
        </SidebarGroupAction>
        <SidebarGroupContent>
          <SidebarMenu>
            {creating && (
              <SidebarMenuItem>
                <div className="flex items-center gap-1 px-2">
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
              </SidebarMenuItem>
            )}
            {folders.map((folder) => (
              <SidebarMenuItem key={folder.id} className="group/folder-item">
                {renamingId === folder.id ? (
                  <div className="flex items-center gap-1 px-2">
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
                  <DroppableMenuItem
                    id={folder.id}
                    isActive={selected === folder.id}
                    onSelect={() => onSelect(folder.id)}
                    onDrop={() => onDropOnFolder(folder.id)}
                    icon={
                      folder.gh_list_id
                        ? <GitFork className="h-4 w-4 text-muted-foreground/60" />
                        : <FolderOpen className="h-4 w-4" />
                    }
                    label={folder.name}
                    badge={folder.star_count ?? undefined}
                  />
                )}
                {renamingId !== folder.id && (
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <SidebarMenuAction
                        showOnHover
                        className="peer-data-[active=true]/menu-button:opacity-100"
                      >
                        <MoreHorizontal className="h-3.5 w-3.5" />
                        <span className="sr-only">Folder actions</span>
                      </SidebarMenuAction>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent side="right" align="start" className="w-36">
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
                )}
              </SidebarMenuItem>
            ))}
          </SidebarMenu>
        </SidebarGroupContent>
      </SidebarGroup>
    </>
  )
}

// ---------------------------------------------------------------------------
// Droppable menu item (dnd-kit useDroppable + SidebarMenuButton)
// ---------------------------------------------------------------------------

function DroppableMenuItem({
  id,
  isActive,
  onSelect,
  onDrop,
  icon,
  label,
  badge,
}: {
  id: FolderSelection
  isActive: boolean
  onSelect: () => void
  onDrop: () => void
  icon: React.ReactNode
  label: string
  badge?: number
}) {
  const { isOver, setNodeRef } = useDroppable({ id: id as string | number })

  return (
    <SidebarMenuItem>
      <SidebarMenuButton
        ref={setNodeRef}
        isActive={isActive}
        onClick={onSelect}
        className={['pr-8', isOver ? 'ring-2 ring-primary ring-inset' : ''].join(' ')}
        // eslint-disable-next-line @typescript-eslint/no-misused-promises
        onDrop={async (e) => { e.preventDefault(); onDrop() }}
        onDragOver={(e) => e.preventDefault()}
      >
        {icon}
        <span className="flex-1 truncate">{label}</span>
        {badge != null && (
          <span className="ml-auto shrink-0 tabular-nums text-[10px] text-sidebar-foreground/60
            group-hover/menu-item:opacity-0 transition-opacity">
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
  const { attributes, listeners, setNodeRef, transform } = useDraggable({ id: star.id })

  const style = transform ? { transform: CSS.Translate.toString(transform) } : undefined

  return (
    <div ref={setNodeRef} style={style} className={isDragging ? 'opacity-30' : undefined}>
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
// Star card content (shared between draggable and DragOverlay)
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
            <DropdownMenuContent align="end" className="w-48">
              {folders
                .filter((f) => f.id !== (typeof selectedFolder === 'number' ? selectedFolder : -1))
                .map((f) => (
                  <DropdownMenuItem key={f.id} onClick={() => onMoveTo?.(f.id)}>
                    <FolderOpen className="mr-2 h-3.5 w-3.5" />Move to {f.name}
                  </DropdownMenuItem>
                ))}
              {typeof selectedFolder === 'number' && (
                <DropdownMenuItem onClick={() => onMoveTo?.('all')}>
                  <X className="mr-2 h-3.5 w-3.5" />Remove from folder
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
                  <Star className="mr-2 h-3.5 w-3.5" />Unstar on GitHub
                </DropdownMenuItem>
              )}
            </DropdownMenuContent>
          </DropdownMenu>
        )}
      </CardContent>
    </Card>
  )
}
