import assert from 'node:assert/strict';

// This checks the original fixture process before any container removal or
// replacement. The fixture records both boundaries inside its own lifecycle.
export async function stopInlineAppGracefully({ docker, id, assertNoSecret }) {
  await docker(['kill', '--signal', 'TERM', id]);
  assert.equal((await docker(['wait', id], { timeout: 30000 })).trim(), '0', 'SIGTERM must produce a normal fixture exit, not a forced removal');
  const state = JSON.parse(await docker(['inspect', '--format', '{{json .State}}', id]));
  assert.equal(state.Running, false);
  assert.equal(state.ExitCode, 0);
  assert.equal(state.OOMKilled, false);
  assert.equal(state.Error, '');
  const logs = await docker(['logs', id]);
  assertNoSecret(logs, 'graceful inline shutdown logs');
  const signal = logs.match(/fixture shutdown signal SIGTERM: worker_children=([1-9][0-9]*), workspaces=([1-9][0-9]*)/);
  assert(signal, 'actual SIGTERM boundary must observe the still-held worker and original workspace');
  const drained = 'fixture graceful shutdown verified: adapters drained, worker_children=0, workspaces=0';
  assert(logs.indexOf(drained) > signal.index, 'original process must verify drained adapters, reaped children and empty workspace before exiting');
  return { heldChildren: Number(signal[1]), heldWorkspaces: Number(signal[2]) };
}
