import test from 'node:test';
import assert from 'node:assert/strict';
import { createHmac } from 'node:crypto';
import { runInNewContext } from 'node:vm';
import { signedInitData, telegramBootstrap, providerAdapter, runCookieBrowserScenarios } from './cookie-browser-scenarios.mjs';

test('synthetic Telegram initData uses the real two-stage WebApp signature', () => {
  const params = new URLSearchParams(signedInitData('test-only-bot-token', 1234, 1700000000));
  const hash = params.get('hash');
  params.delete('hash');
  const checkString = [...params.keys()].sort().map(key => `${key}=${params.get(key)}`).join('\n');
  const key = createHmac('sha256', 'WebAppData').update('test-only-bot-token').digest();
  assert.equal(hash === createHmac('sha256', key).update(checkString).digest('hex'), true);
  assert.equal(params.get('auth_date'), '1700000000');
  assert.equal(JSON.parse(params.get('user')).id, 1234);
  assert.equal(params.get('start_param'), 'cookies');
});

function bootstrap(cached = false) {
  const values = new Map(cached ? [['fixture-cached-auth', '1']] : []);
  class Xhr {
    addEventListener(type, handler) { this.handler = handler; }
    open(method, url) { this.original = { method, url }; }
  }
  const response = { status: 200, untouched: true };
  const context = { URL, XMLHttpRequest: Xhr, location: { href: 'https://fixture.test/cookies' },
    localStorage: { setItem(k, v) { values.set(k, v); } }, sessionStorage: { getItem(k) { return values.get(k); } },
    fetch: async () => response };
  context.window = context;
  runInNewContext(telegramBootstrap('signed-test-init-data', 1234), context);
  return { context, response, Xhr };
}

test('bootstrap supports actual provider methods and cached reload without replacing responses', async () => {
  const { context, response } = bootstrap();
  const app = context.Telegram.WebApp;
  assert.equal(app.initData === 'signed-test-init-data', true, 'bootstrap has signed initData');
  app.ready(); app.expand();
  assert.equal(context.__fixtureReady && context.__fixtureExpanded, true);
  app.CloudStorage.getItems(['lang'], (error, values) => { assert.equal(error, null); assert.equal(values.lang, 'en'); });
  assert.equal(await context.fetch('/api/v1/auth/token'), response);
  assert.equal(context.__fixtureResponses[0].kind, 'auth');
  assert.equal(bootstrap(true).context.Telegram.WebApp.initData, '');
});

test('XHR observes only real status and poll state, excluding provider and cookie secrets', () => {
  const { context, Xhr } = bootstrap();
  const xhr = new Xhr();
  xhr.open('GET', '/api/v1/dl/cookies/yttv/poll/private-session');
  xhr.status = 200;
  xhr.responseType = '';
  xhr.responseText = JSON.stringify({ status: 'pending', secret: 'private-provider-token' });
  xhr.handler();
  assert.equal(xhr.original.url.endsWith('/private-session'), true);
  assert.equal(context.__fixtureResponses[0].state, 'pending');
  const diagnostics = JSON.stringify(context.__fixtureResponses);
  assert.equal(diagnostics.includes('private-'), false);
  assert.equal(diagnostics.includes('responseText'), false);
});

test('provider boundary fails closed instead of permitting real provider calls', () => {
  assert.throws(() => providerAdapter({}), /native offline provider control adapter/);
  assert.throws(() => providerAdapter({ cookieBrowserProviders: { enable() {} } }), /adapter/);
});

test('partial offline-provider enable failure restores only owned state, without launching browser', async () => {
  const statements = [];
  let restored = false;
  const context = { origin: 'https://fixture.test', registerCookieSyncToken() {},
    async fixtureSql(sql) {
      statements.push(sql);
      if (sql.startsWith('SELECT count(*) FROM users')) return '0';
      if (sql.includes('pragma_table_info')) return '["user_id"]';
      if (sql.includes('json_group_array(json_object')) return '[]';
      return '';
    },
    cookieBrowserProviders: {
      async enable() { throw new Error('sensitive-provider-response'); },
      set() {}, counts() {}, async restore() { restored = true; },
    } };
  await assert.rejects(runCookieBrowserScenarios(context), { message: 'Cookie browser scenario execution failed' });
  assert.equal(restored, true);
  assert.equal(statements.some(sql => sql === 'DELETE FROM users WHERE id IN (1900000042,1900000099)'), true);
  assert.equal(statements.filter(sql => sql.startsWith('DELETE FROM')).every(sql => /(?:user_id|actor_id|id) IN \(1900000042,1900000099\)/.test(sql)), true);
});
