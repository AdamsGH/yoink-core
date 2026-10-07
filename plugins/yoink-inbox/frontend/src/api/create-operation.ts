import type { InboxGhFolderCreate } from '../types/index'

export interface CreateOperation {
  key: string
  body: InboxGhFolderCreate
  mutationId?: string
}
const storageKey = (user: string) => `inbox:remote-create:${user}`
export function readOperation(storage: Pick<Storage, 'getItem'>, user: string): CreateOperation | null {
  const raw = storage.getItem(storageKey(user))
  if (!raw) return null
  const op = JSON.parse(raw) as CreateOperation
  if (typeof op?.key !== 'string' || !op.key || op.key.length > 128 ||
      !op.body || typeof op.body.name !== 'string' || op.body.is_local !== false ||
      (op.mutationId !== undefined && typeof op.mutationId !== 'string')) {
    throw new Error('Invalid saved create')
  }
  return op
}
export function saveOperation(storage: Pick<Storage, 'setItem'>, user: string, op: CreateOperation): void {
  storage.setItem(storageKey(user), JSON.stringify(op))
}
export function finishOperation(storage: Pick<Storage, 'removeItem'>, user: string): void {
  storage.removeItem(storageKey(user))
}
export function newOperation(body: InboxGhFolderCreate, uuid: () => string): CreateOperation {
  return { key: uuid(), body: { ...body } }
}
export function isApplied(state: string): boolean { return state === 'applied' }
