import assert from 'node:assert/strict';

export function verifyNativeFailure(state, logs, reason) {
  assert.equal(state.Status, 'exited', logs);
  assert.equal(state.Running, false);
  assert.equal(state.OOMKilled, false);
  assert.equal(state.Error, '', 'native command must run, not fail during container startup');
  assert(Date.parse(state.StartedAt) > 0, 'native command was actually started');
  assert.equal(state.ExitCode, 1, logs);
  assert.match(logs, reason, 'native rejection reason, not a Docker wrapper error');
}
