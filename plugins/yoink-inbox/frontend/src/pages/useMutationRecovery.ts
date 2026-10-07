import { useGetIdentity } from '@refinedev/core'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { createFolder, listFolders } from '../api/items'
import { finishOperation, newOperation, readOperation, saveOperation } from '../api/create-operation'
import type { CreateOperation } from '../api/create-operation'
import { listMutations, mutationStatus, pendingMutationId } from '../api/mutations'
import type { MutationStatus } from '../api/mutations'
import type { InboxGhFolder, InboxGhFolderCreate } from '../types'

export function useMutationRecovery(onFolders: (folders: InboxGhFolder[]) => void) {
  const { data: identity } = useGetIdentity<{ id: string }>()
  const user = identity?.id ? String(identity.id) : null
  const { t } = useTranslation()
  const [operation, setOperation] = useState<CreateOperation | null>(null)
  const [mutations, setMutations] = useState<MutationStatus[]>([])
  const [unavailable, setUnavailable] = useState(false)
  const [busy, setBusy] = useState(false)
  const locked = useRef(false)
  const refreshGeneration = useRef(0)
  const currentUser = useRef(user)
  currentUser.current = user

  const refresh = useCallback(async () => {
    if (!user) return
    const version = ++refreshGeneration.current
    const token = localStorage.getItem('access_token')
    try {
      const saved = readOperation(localStorage, user)
      const active = await listMutations()
      const ids = new Set(active.map((m) => m.id))
      if (saved?.mutationId) ids.add(saved.mutationId)
      const statuses = await Promise.all([...ids].map((id) =>
        mutationStatus(id, id === saved?.mutationId ? saved.key : undefined)))
      if (currentUser.current !== user || version !== refreshGeneration.current || localStorage.getItem('access_token') !== token) return
      // A conflict ID may identify another request. Only key-match proof permits
      // discarding a rejected request; applied still needs keyed response retrieval.
      setOperation(saved)
      setMutations(statuses)
      setUnavailable(false)
    } catch {
      if (currentUser.current === user && version === refreshGeneration.current) setUnavailable(true)
    }
  }, [user])

  useEffect(() => {
    setOperation(null)
    setMutations([])
    setUnavailable(false)
    if (!user) return
    try { setOperation(readOperation(localStorage, user)) } catch { setUnavailable(true) }
    void refresh()
    return () => { ++refreshGeneration.current }
  }, [user, refresh])

  async function send(op: CreateOperation): Promise<InboxGhFolder | null> {
    if (!user || locked.current) return null
    const token = localStorage.getItem('access_token')
    locked.current = true
    ++refreshGeneration.current
    setBusy(true)
    try {
      // Persist before network admission. Storage failures must prevent the write.
      saveOperation(localStorage, user, op)
      setOperation(op)
      // Recovery endpoints are mandatory for safe native writes. Legacy deployments
      // without them remain untouched rather than accepting an unverifiable retry.
      await listMutations()
      if (currentUser.current !== user || localStorage.getItem('access_token') !== token) return null
      const folder = await createFolder(op.body, op.key)
      // Legacy responses must not be mistaken for native confirmed remote success.
      if (!folder.gh_list_id) throw new Error('Remote create not confirmed')
      ++refreshGeneration.current
      finishOperation(localStorage, user)
      if (currentUser.current !== user) return null
      setOperation(null)
      const folders = await listFolders()
      if (currentUser.current !== user) return null
      onFolders(folders)
      toast.success(t('inboxRecovery.applied'))
      return folder
    } catch (error) {
      ++refreshGeneration.current
      const id = pendingMutationId(error)
      if (id) {
        const pending = { ...op, mutationId: id }
        try { saveOperation(localStorage, user, pending) } catch { /* original identity remains durable */ }
        if (currentUser.current === user) setOperation(pending)
      }
      if (currentUser.current === user) {
        toast.error(t('inboxRecovery.unknown'))
        await refresh()
      }
      return null
    } finally {
      locked.current = false
      if (currentUser.current === user) setBusy(false)
    }
  }

  async function create(body: InboxGhFolderCreate): Promise<InboxGhFolder | null> {
    if (!user) { toast.error(t('inboxRecovery.unavailable')); return null }
    // An intentional identical create gets a new UUID only after the prior operation is resolved.
    try {
      // Serialize admission across tabs as well as within this mounted hook.
      // Without Web Locks, fail closed instead of overwriting another saved key.
      if (!navigator.locks) throw new Error('Durable create admission unavailable')
      return await navigator.locks.request(`inbox:remote-create:${user}`, async () => {
        if (currentUser.current !== user) return null
        if (readOperation(localStorage, user)) {
          toast.error(t('inboxRecovery.resolveFirst'))
          return null
        }
        return send(newOperation(body, () => crypto.randomUUID()))
      })
    } catch {
      toast.error(t('inboxRecovery.unavailable'))
      return null
    }
  }

  async function retry() {
    if (!operation) return
    if (operation.mutationId && !mutations.some((m) => m.id === operation.mutationId && ['applied', 'rejected'].includes(m.state))) return
    // Explicit retry only, with the exact saved body/key. Never a fresh operation.
    try {
      if (!user || !navigator.locks) throw new Error('Durable create admission unavailable')
      await navigator.locks.request(`inbox:remote-create:${user}`, async () => {
        if (currentUser.current !== user) return
        const saved = readOperation(localStorage, user)
        if (saved?.key !== operation.key) { await refresh(); return }
        await send(saved)
      })
    } catch { toast.error(t('inboxRecovery.unavailable')) }
  }

  async function discardRejected() {
    if (!user || !operation?.mutationId || !mutations.some((m) => m.id === operation.mutationId && m.state === 'rejected' && m.operation_key_matches === true)) return
    try {
      if (!navigator.locks) throw new Error('Durable create admission unavailable')
      await navigator.locks.request(`inbox:remote-create:${user}`, async () => {
        if (currentUser.current !== user) return
        const saved = readOperation(localStorage, user)
        if (!saved?.mutationId || saved.key !== operation.key || saved.mutationId !== operation.mutationId) { await refresh(); return }
        // Revalidate proof for this exact persisted key/ID under the admission lock.
        // Missing proof (including older servers) must retain the uncertain key.
        const status = await mutationStatus(saved.mutationId, saved.key)
        if (currentUser.current !== user || status.id !== saved.mutationId ||
            status.state !== 'rejected' || status.operation_key_matches !== true) {
          await refresh()
          return
        }
        ++refreshGeneration.current
        finishOperation(localStorage, user)
        if (currentUser.current === user) setOperation(null)
      })
    } catch { toast.error(t('inboxRecovery.unavailable')) }
  }

  async function recovered() {
    const owner = user
    await refresh()
    const folders = await listFolders()
    if (owner === currentUser.current) onFolders(folders)
  }

  return { create, operation, mutations, unavailable, busy, refresh, retry, recovered, discardRejected }
}
