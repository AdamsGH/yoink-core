import { ChevronDown, ChevronRight, Trash2, UserMinus, UserPlus } from 'lucide-react'
import { useEffect, useState } from 'react'
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
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  IconButton,
  SkeletonList,
  Textarea,
} from '@ui'
import { CompactCardHeader, EmptyState } from '@app'

import {
  addTeamMember,
  createTeam,
  deleteTeam,
  listTeams,
  removeTeamMember,
  patchTeamMember,
} from '@inbox/api/items'
import type { InboxTeam, InboxTeamCreate } from '@inbox/types'

const emptyForm = (): InboxTeamCreate => ({ name: '', description: '' })

export default function TeamsPage() {
  const [teams, setTeams] = useState<InboxTeam[]>([])
  const [loading, setLoading] = useState(true)
  const [expanded, setExpanded] = useState<Set<number>>(new Set())
  const [creating, setCreating] = useState(false)
  const [form, setForm] = useState<InboxTeamCreate>(emptyForm())

  // per-team invite form state
  const [inviteUserId, setInviteUserId] = useState<Record<number, string>>({})
  const [inviteRole, setInviteRole] = useState<Record<number, string>>({})

  useEffect(() => {
    listTeams()
      .then(setTeams)
      .catch((err) => {
        if (import.meta.env.DEV) console.error(err)
        toast.error('Failed to load teams')
      })
      .finally(() => setLoading(false))
  }, [])

  function toggleExpand(id: number) {
    setExpanded((prev) => {
      const next = new Set(prev)
      next.has(id) ? next.delete(id) : next.add(id)
      return next
    })
  }

  async function onCreate() {
    if (!form.name.trim()) return
    try {
      const team = await createTeam(form)
      setTeams((p) => [team, ...p])
      setForm(emptyForm())
      setCreating(false)
      toast.success('Team created')
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to create team')
    }
  }

  async function onDelete(id: number) {
    try {
      await deleteTeam(id)
      setTeams((p) => p.filter((t) => t.id !== id))
      toast.success('Team deleted')
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to delete team')
    }
  }

  async function onInvite(team: InboxTeam) {
    const rawId = inviteUserId[team.id] ?? ''
    const uid = parseInt(rawId, 10)
    if (!uid) { toast.error('Enter a numeric user ID'); return }
    const role = inviteRole[team.id] ?? 'member'
    try {
      const member = await addTeamMember(team.id, uid, role)
      setTeams((p) => p.map((t) =>
        t.id === team.id ? { ...t, members: [...t.members, member] } : t
      ))
      setInviteUserId((p) => ({ ...p, [team.id]: '' }))
      toast.success('Member added')
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to add member')
    }
  }

  async function onRemoveMember(teamId: number, userId: number) {
    try {
      await removeTeamMember(teamId, userId)
      setTeams((p) => p.map((t) =>
        t.id === teamId ? { ...t, members: t.members.filter((m) => m.user_id !== userId) } : t
      ))
      toast.success('Member removed')
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to remove member')
    }
  }

  async function onChangeRole(teamId: number, userId: number, role: string) {
    try {
      const updated = await patchTeamMember(teamId, userId, role)
      setTeams((p) => p.map((t) =>
        t.id === teamId
          ? { ...t, members: t.members.map((m) => m.user_id === userId ? { ...m, role: updated.role } : m) }
          : t
      ))
    } catch (err) {
      if (import.meta.env.DEV) console.error(err)
      toast.error('Failed to change role')
    }
  }

  return (
    <div className="container mx-auto max-w-3xl space-y-4 py-4">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold">Inbox Teams</h1>
        <Button onClick={() => setCreating((c) => !c)}>
          {creating ? 'Cancel' : 'New team'}
        </Button>
      </div>

      {creating && (
        <Card>
          <CardContent className="space-y-3 p-4">
            <div className="space-y-1">
              <Label>Name</Label>
              <Input value={form.name} onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))} placeholder="Team name" />
            </div>
            <div className="space-y-1">
              <Label>Description</Label>
              <Textarea value={form.description ?? ''} onChange={(e) => setForm((f) => ({ ...f, description: e.target.value }))} rows={2} placeholder="Optional" />
            </div>
            <Button onClick={onCreate} size="sm">Create</Button>
          </CardContent>
        </Card>
      )}

      {loading ? (
        <SkeletonList count={3}>{(i) => <Card key={i}><CardContent className="h-16" /></Card>}</SkeletonList>
      ) : teams.length === 0 ? (
        <EmptyState message="No teams yet. Create one to share categories with others." />
      ) : (
        <div className="space-y-2">
          {teams.map((team) => {
            const isOpen = expanded.has(team.id)
            return (
              <Card key={team.id}>
                <CompactCardHeader
                  title={
                    <button
                      className="flex items-center gap-2 text-left font-medium"
                      onClick={() => toggleExpand(team.id)}
                    >
                      {isOpen ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />}
                      {team.name}
                      <span className="text-xs text-muted-foreground">
                        {team.members.length} {team.members.length === 1 ? 'member' : 'members'}
                      </span>
                    </button>
                  }
                  actions={
                    <AlertDialog>
                      <AlertDialogTrigger asChild>
                        <IconButton tooltip="Delete team" variant="destructive"><Trash2 className="h-4 w-4" /></IconButton>
                      </AlertDialogTrigger>
                      <AlertDialogContent>
                        <AlertDialogHeader>
                          <AlertDialogTitle>Delete team "{team.name}"?</AlertDialogTitle>
                          <AlertDialogDescription>Members will lose access to shared categories.</AlertDialogDescription>
                        </AlertDialogHeader>
                        <AlertDialogFooter>
                          <AlertDialogCancel>Cancel</AlertDialogCancel>
                          <AlertDialogAction onClick={() => onDelete(team.id)}>Delete</AlertDialogAction>
                        </AlertDialogFooter>
                      </AlertDialogContent>
                    </AlertDialog>
                  }
                />
                {isOpen && (
                  <CardContent className="space-y-3 pt-0">
                    {/* Member list */}
                    <div className="divide-y">
                      {team.members.map((m) => (
                        <div key={m.user_id} className="flex items-center gap-3 py-2">
                          <span className="flex-1 text-sm font-mono">uid:{m.user_id}</span>
                          <Select
                            value={m.role}
                            onValueChange={(r) => onChangeRole(team.id, m.user_id, r)}
                          >
                            <SelectTrigger className="h-7 w-24 text-xs">
                              <SelectValue />
                            </SelectTrigger>
                            <SelectContent>
                              <SelectItem value="owner">owner</SelectItem>
                              <SelectItem value="admin">admin</SelectItem>
                              <SelectItem value="member">member</SelectItem>
                            </SelectContent>
                          </Select>
                          <Badge variant={m.role === 'owner' ? 'default' : 'secondary'}>{m.role}</Badge>
                          {m.role !== 'owner' && (
                            <IconButton tooltip="Remove" onClick={() => onRemoveMember(team.id, m.user_id)}><UserMinus className="h-3.5 w-3.5" /></IconButton>
                          )}
                        </div>
                      ))}
                    </div>
                    {/* Invite */}
                    <div className="flex items-end gap-2">
                      <div className="flex-1 space-y-1">
                        <Label className="text-xs">User ID</Label>
                        <Input
                          className="h-8"
                          placeholder="123456789"
                          value={inviteUserId[team.id] ?? ''}
                          onChange={(e) => setInviteUserId((p) => ({ ...p, [team.id]: e.target.value }))}
                        />
                      </div>
                      <div className="space-y-1">
                        <Label className="text-xs">Role</Label>
                        <Select
                          value={inviteRole[team.id] ?? 'member'}
                          onValueChange={(r) => setInviteRole((p) => ({ ...p, [team.id]: r }))}
                        >
                          <SelectTrigger className="h-8 w-24">
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectItem value="admin">admin</SelectItem>
                            <SelectItem value="member">member</SelectItem>
                          </SelectContent>
                        </Select>
                      </div>
                      <Button size="sm" className="h-8" onClick={() => onInvite(team)}>
                        <UserPlus className="mr-1.5 h-3.5 w-3.5" />
                        Invite
                      </Button>
                    </div>
                  </CardContent>
                )}
              </Card>
            )
          })}
        </div>
      )}
    </div>
  )
}

