import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { Button, Card, CardContent, AlertDialog, AlertDialogContent, AlertDialogHeader, AlertDialogTitle, AlertDialogDescription, AlertDialogFooter, AlertDialogCancel, AlertDialogAction } from '@ui'
import { EmptyState } from '@app'
import { adoptCreate, cancelMutation, createCandidates, reconcileMutation } from '../api/mutations'
import type { CreateCandidate, MutationStatus } from '../api/mutations'
import type { useMutationRecovery } from './useMutationRecovery'

type Recovery = ReturnType<typeof useMutationRecovery>
export function MutationRecovery({ recovery }: { recovery: Recovery }) {
  const { t } = useTranslation()
  const [busy, setBusy] = useState(false)
  const [candidates, setCandidates] = useState<Record<string, CreateCandidate[]>>({})
  const [selection, setSelection] = useState<{ mutation: string; candidate: CreateCandidate } | null>(null)
  async function action(run: () => Promise<unknown>) {
    setBusy(true)
    try { await run(); await recovery.recovered() }
    catch { toast.error(t('inboxRecovery.unknown')); await recovery.refresh() }
    finally { setBusy(false) }
  }
  if (!recovery.operation && !recovery.mutations.length && !recovery.unavailable) return null
  return <Card><CardContent className="space-y-3 p-4">
    <h2 className="font-medium">{t('inboxRecovery.title')}</h2>
    <p className="text-sm text-muted-foreground">{t(recovery.unavailable ? 'inboxRecovery.unavailable' : 'inboxRecovery.unknown')}</p>
    <Button size="sm" variant="outline" disabled={busy || recovery.busy} onClick={() => void recovery.refresh()}>{t('inboxRecovery.refresh')}</Button>
    {recovery.operation && (!recovery.operation.mutationId || recovery.mutations.some((m) => m.id === recovery.operation?.mutationId && ['applied', 'rejected'].includes(m.state))) && <div className="space-y-2">
      <p>{recovery.operation.body.name}</p>
      <Button size="sm" disabled={busy || recovery.busy} onClick={() => void recovery.retry()}>{t('inboxRecovery.retry')}</Button>
    </div>}
    {recovery.operation?.mutationId && recovery.mutations.some((m) => m.id === recovery.operation?.mutationId && m.state === 'rejected' && m.operation_key_matches === true) && <Button size="sm" variant="outline" disabled={busy || recovery.busy} onClick={recovery.discardRejected}>{t('inboxRecovery.discardRejected')}</Button>}
    {recovery.mutations.map((m: MutationStatus) => <div key={m.id} className="space-y-2 rounded border p-3">
      <p className="break-all text-sm">{m.id} - {t(`inboxRecovery.states.${m.state}`)}</p>
      {['sending', 'unknown', 'confirmed'].includes(m.state) && <Button size="sm" variant="outline" disabled={busy} onClick={() => void action(() => reconcileMutation(m.id))}>{t('inboxRecovery.reconcile')}</Button>}
      {m.can_cancel && <Button size="sm" variant="outline" disabled={busy} onClick={() => void action(() => cancelMutation(m.id))}>{t('inboxRecovery.cancel')}</Button>}
      {m.can_inspect_candidates && <Button size="sm" variant="outline" disabled={busy} onClick={() => void action(async () => { setCandidates((prev) => { const next = { ...prev }; delete next[m.id]; return next }); const rows = await createCandidates(m.id); setCandidates((prev) => ({ ...prev, [m.id]: rows })) })}>{t('inboxRecovery.inspect')}</Button>}
      {m.can_inspect_candidates && candidates[m.id]?.length === 0 && <EmptyState message={t('inboxRecovery.noCandidates')} />}
      {m.can_inspect_candidates && candidates[m.id]?.map((c) => <div key={c.id} className="space-y-1">
        <p className="break-all text-sm">{c.name} - {c.id} - {c.slug}</p>
        {c.description && <p className="text-sm">{c.description}</p>}
        <Button size="sm" variant="outline" disabled={busy} onClick={() => setSelection({ mutation: m.id, candidate: c })}>{t('inboxRecovery.adopt')}</Button>
      </div>)}
    </div>)}
    <AlertDialog open={selection !== null} onOpenChange={(open) => { if (!open) setSelection(null) }}>
      <AlertDialogContent><AlertDialogHeader>
        <AlertDialogTitle>{t('inboxRecovery.adoptTitle')}</AlertDialogTitle>
        <AlertDialogDescription>{t('inboxRecovery.adoptDescription', { id: selection?.candidate.id })}</AlertDialogDescription>
      </AlertDialogHeader><AlertDialogFooter>
        <AlertDialogCancel>{t('common.cancel')}</AlertDialogCancel>
        <AlertDialogAction onClick={() => { if (selection) void action(() => adoptCreate(selection.mutation, selection.candidate.id)) }}>{t('inboxRecovery.adopt')}</AlertDialogAction>
      </AlertDialogFooter></AlertDialogContent>
    </AlertDialog>
  </CardContent></Card>
}
