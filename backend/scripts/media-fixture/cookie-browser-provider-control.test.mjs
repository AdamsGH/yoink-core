import assert from 'node:assert/strict';
import test from 'node:test';
import { createCookieBrowserProviderControl } from './cookie-browser-provider-control.mjs';

const state = { oauth_mode: 'pending', account_mode: 'valid', oauth_start_calls: 2, oauth_poll_calls: 3, oauth_refresh_calls: 4, account_fetch_calls: 5 };

test('offline control reads counters, limits modes and restores its reserved row', async () => {
  const queries = [];
  const control = createCookieBrowserProviderControl(async sql => {
    queries.push(sql);
    return sql.startsWith('SELECT') ? JSON.stringify(state) : '';
  });
  await assert.rejects(control.set({ oauth: 'ok', account: 'valid' }), /must be enabled/);
  assert.deepEqual(await control.enable(), { enabled: true });
  await assert.rejects(control.enable(), /already active/);
  assert.deepEqual(await control.counts(), { start: 2, poll: 3, refresh: 4, validate: 5 });
  const before = queries.length;
  await assert.rejects(control.set({ oauth: "bad'", account: 'valid' }), /unsupported/);
  assert.equal(queries.length, before);
  await control.set({ oauth: 'ok', account: 'error' });
  assert.match(queries.at(-1), /json_set\(value,'\$\.oauth_mode','ok','\$\.account_mode','error'\)/);
  await control.restore();
  assert.equal(queries.at(-1), `UPDATE bot_settings SET value='${JSON.stringify(state)}' WHERE key='fixture_cookie_provider_control'`);
  const restored = queries.length;
  await control.restore();
  assert.equal(queries.length, restored);
});
