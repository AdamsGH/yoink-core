import test from 'node:test';
import assert from 'node:assert/strict';
import { formatFixtureRows, createNativeDbClient } from './native-db.mjs';

test('fixture rows preserve null-as-empty and stable pipe/newline formatting', () => {
  assert.equal(formatFixtureRows([[1, null, 'x'], [false, 2]]), '1||x\nfalse|2');
});

test('native SQL client enforces payload bound before invoking transport', async () => {
  let helperCalls = 0;
  const client = createNativeDbClient({
    getAppContainerId: () => undefined,
    getFixturePort: () => 0,
    fixtureToken: 'fixture',
    volumeName: 'volume',
    fixtureImage: 'image',
    maxPayload: 32,
    helperContainer: async () => { helperCalls++; return '{}'; },
  });
  await assert.rejects(client.request('x'.repeat(100)), /fixture request exceeds bound/);
  assert.equal(helperCalls, 0);
});
