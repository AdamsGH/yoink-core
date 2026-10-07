import assert from 'node:assert/strict';

const oauthModes = new Set(['pending', 'ok', 'expired', 'error', 'start_error']);
const accountModes = new Set(['valid', 'invalid', 'error']);
const columns = ['oauth_mode', 'account_mode', 'oauth_start_calls', 'oauth_poll_calls', 'oauth_refresh_calls', 'account_fetch_calls'];
const literal = value => typeof value === 'number' ? String(value) : `'${value}'`;

export function createCookieBrowserProviderControl(fixtureSql) {
  let original;
  const read = async () => JSON.parse(await fixtureSql("SELECT value FROM bot_settings WHERE key='fixture_cookie_provider_control'"));
  return {
    async enable() {
      assert(!original, 'offline provider control is already active');
      const saved = await read();
      assert(saved && oauthModes.has(saved.oauth_mode) && accountModes.has(saved.account_mode), 'offline provider control must be initialized by the fixture binary');
      assert(Object.keys(saved).length === columns.length && columns.every(column => Object.hasOwn(saved, column)), 'offline provider control has unsupported fields');
      assert(columns.slice(2).every(column => Number.isSafeInteger(saved[column]) && saved[column] >= 0), 'offline provider counters must be nonnegative integers');
      original = saved;
      return { enabled: true };
    },
    async set({ oauth, account }) {
      assert(original, 'offline provider control must be enabled');
      assert(oauthModes.has(oauth) && accountModes.has(account), 'unsupported offline provider mode');
      await fixtureSql(`UPDATE bot_settings SET value=json_set(value,'$.oauth_mode',${literal(oauth)},'$.account_mode',${literal(account)}) WHERE key='fixture_cookie_provider_control'`);
    },
    async counts() {
      const row = await read();
      return { start: row.oauth_start_calls, poll: row.oauth_poll_calls, validate: row.account_fetch_calls, refresh: row.oauth_refresh_calls };
    },
    async restore() {
      if (!original) return;
      await fixtureSql(`UPDATE bot_settings SET value='${JSON.stringify(original)}' WHERE key='fixture_cookie_provider_control'`);
      original = undefined;
    },
  };
}
