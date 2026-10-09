import http from 'node:http';
import { createHash } from 'node:crypto';

const BASE = '/api/v1/dl/cookies';
const check = (ok, label) => { if (!ok) throw new Error(`Cookie lifecycle assertion failed: ${label}`); };
const hash = value => createHash('sha256').update(value).digest('hex');
const literal = value => `'${String(value).replaceAll("'", "''")}'`;

function loopback(base) {
  const url = new URL(base);
  check(url.protocol === 'http:' && url.hostname === '127.0.0.1' && url.pathname === '/' && !url.username && !url.password && !url.search && !url.hash, 'native loopback HTTP boundary');
}

/** Both HTTP requests are outstanding with incomplete bodies before either is released.
 * Separate sockets and no bearer credentials exercise public single-use arbitration.
 */
export async function concurrentCookieSubmit(base, bodies) {
  loopback(base);
  check(bodies.length === 2, 'two concurrent contenders');
  const requests = [];
  try {
    const held = bodies.map(body => {
      const bytes = Buffer.from(JSON.stringify(body));
      let release;
      const ready = new Promise(resolve => { release = resolve; });
      const done = new Promise((resolve, reject) => {
        const req = http.request(`${base}${BASE}/submit`, { method: 'POST', agent: false, headers: { 'content-type': 'application/json', 'content-length': bytes.length, expect: '100-continue' } }, res => {
          res.resume();
          res.once('end', () => resolve(res.statusCode));
          res.once('error', reject);
        });
        requests.push(req);
        req.once('error', reject);
        req.setTimeout(15000, () => req.destroy(new Error('Cookie lifecycle concurrent request deadline')));
        // The server must admit both body readers before we release either body.
        req.once('continue', () => req.write(bytes.subarray(0, bytes.length - 1), release));
        req.flushHeaders();
      });
      // Attach immediately, including when a socket fails before the barrier opens.
      done.catch(() => {});
      return { ready, done, tail: bytes.subarray(bytes.length - 1) };
    });
    await Promise.race([Promise.all(held.map(h => h.ready)), ...held.map(h => h.done.then(() => { throw new Error('Cookie lifecycle premature response'); }))]);
    held.forEach((h, i) => requests[i].end(h.tail));
    return await Promise.all(held.map(h => h.done));
  } finally { requests.forEach(req => req.destroy()); }
}

/** Runs only inside the browser runner's disposable actor/provider scope. */
export async function runCookieLifecycleScenarios(context) {
  const { appHttpPort, fixtureSql, restartApp, providers, actorId, foreignId, jwt, foreignJwt, registerCookieSyncToken } = context;
  const base = `http://127.0.0.1:${appHttpPort}`;
  loopback(base);
  check(typeof restartApp === 'function', 'same-volume current-image restart required');
  async function request(path, method = 'GET', body, bearer = jwt) {
    const r = await fetch(base + path, { method, headers: { 'content-type': 'application/json', ...(bearer ? { authorization: `Bearer ${bearer}` } : {}) }, ...(body === undefined ? {} : { body: JSON.stringify(body) }), signal: AbortSignal.timeout(15000), redirect: 'error' });
    const text = await r.text();
    return { status: r.status, data: text ? JSON.parse(text) : null };
  }
  async function expect(path, method, body, status, bearer = jwt) {
    const r = await request(path, method, body, bearer);
    check(r.status === status, `${method} route expected ${status}, observed ${r.status}`);
    return r.data;
  }
  async function issue(bearer = jwt) {
    const r = await expect(`${BASE}/token`, 'POST', undefined, 200, bearer);
    registerCookieSyncToken(r.token);
    check(/^[a-f0-9]{64}$/.test(r.token) && r.expires_in === 600 && r.submit_url === `${BASE}/submit`, 'issued token contract');
    return r.token;
  }
  const cookie = (domain, value = 'synthetic-lifecycle') => ({ domain: `.${domain}`, path: '/', name: 'SID', value, secure: true, httpOnly: true, expirationDate: 4102444800 });
  const body = (token, domain, value) => ({ token, cookies: { [domain]: [cookie(domain, value)] } });
  const submit = (payload, status = 204, bearer = null) => expect(`${BASE}/submit`, 'POST', payload, status, bearer);
  const owned = () => fixtureSql(`SELECT * FROM cookies WHERE user_id IN (${actorId},${foreignId}) ORDER BY id`).then(hash);
  const tokenCount = () => fixtureSql(`SELECT count(*) FROM cookie_web_tokens WHERE user_id=${actorId}`);
  const tokenRow = token => `token_hash=${literal(hash(token))} AND user_id=${actorId}`;
  const sessionRow = session => `session_hash=${literal(hash(session))} AND user_id=${actorId}`;
  async function start() {
    const r = await expect(`${BASE}/yttv/start`, 'POST', undefined, 200);
    registerCookieSyncToken(r.session_id);
    check(/^[a-f0-9]{64}$/.test(r.session_id) && r.interval === 1 && r.expires_in === 1800, 'device start contract');
    return r.session_id;
  }
  async function poll(session, state, bearer = jwt, status = 200) {
    const r = await expect(`${BASE}/yttv/poll/${session}`, 'GET', undefined, status, bearer);
    if (state) check(r.status === state, `device poll ${state}`);
  }
  await expect(`${BASE}/token`, 'DELETE', undefined, 204);
  const a = await issue(), b = await issue(), foreign = await issue(foreignJwt);
  check(new Set([a, b, foreign]).size === 3, 'independent tokens');
  await submit(body(a, 'lifecycle-a.test'), 204, foreignJwt);
  check(await fixtureSql(`SELECT count(*) FROM cookies WHERE user_id=${actorId} AND domain='lifecycle-a.test'`) === '1', 'token actor overrides foreign bearer');
  await submit(body(foreign, 'lifecycle-foreign.test'), 204, jwt);
  check(await fixtureSql(`SELECT count(*) FROM cookies WHERE user_id=${foreignId} AND domain='lifecycle-foreign.test'`) === '1', 'foreign token remains foreign');
  check(await tokenCount() === '1', 'other token survives consumption');
  const beforeSize = await owned();
  await submit(body(b, 'lifecycle-b.test', 'x'.repeat(512 * 1024)), 422);
  check(await owned() === beforeSize && await tokenCount() === '1', 'oversize rollback retains token');
  await fixtureSql(`UPDATE cookie_web_tokens SET expires_at=unixepoch()-1 WHERE ${tokenRow(b)}`);
  await submit(body(b, 'lifecycle-b.test'), 401);
  check(await owned() === beforeSize, 'expired token has no writes');
  const race = await issue();
  const statuses = await concurrentCookieSubmit(base, [body(race, 'lifecycle-race.test', 'contender-a'), body(race, 'lifecycle-race.test', 'contender-b')]);
  check(statuses.slice().sort((x, y) => x - y).join(',') === '204,401', 'exactly one concurrent submit wins');
  check(await fixtureSql(`SELECT count(*) FROM cookies WHERE user_id=${actorId} AND domain='lifecycle-race.test'`) === '1', 'single winner row');
  const winner = await fixtureSql(`SELECT content FROM cookies WHERE user_id=${actorId} AND domain='lifecycle-race.test'`);
  check(winner.includes(statuses[0] === 204 ? 'contender-a' : 'contender-b'), 'stored content matches sole winner');
  const pending = await issue();
  await restartApp();
  await submit(body(race, 'lifecycle-race.test'), 401);
  await submit(body(pending, 'lifecycle-restart.test'));
  await restartApp();
  const persisted = await owned();
  await submit(body(pending, 'lifecycle-restart.test'), 401);
  check(await owned() === persisted, 'restart preserves writes and replay rejection');
  await expect(`${BASE}/token`, 'DELETE', undefined, 204);
  const revoked = await issue();
  for (let i = 1; i < 128; i++) await issue();
  await expect(`${BASE}/token`, 'POST', undefined, 429);
  check(await tokenCount() === '128', 'token capacity has no extra row');
  await expect(`${BASE}/token`, 'DELETE', undefined, 204);
  check(await tokenCount() === '0', 'revocation clears actor tokens');
  const beforeRevokeReplay = await owned();
  await submit(body(revoked, 'lifecycle-revoked.test'), 401);
  check(await owned() === beforeRevokeReplay, 'revoked token cannot mutate');

  await providers.set({ actorId, oauth: 'pending', account: 'valid' });
  // UI cancel is client-only. Starting a replacement does not revoke the old session.
  const first = await start(), replacement = await start();
  check(first !== replacement, 'replacement independently issued');
  const counts = await providers.counts({ actorId });
  await poll(first, null, foreignJwt, 403);
  check((await providers.counts({ actorId })).poll === counts.poll, 'foreign device poll never reaches provider');
  await poll(first, 'pending');
  await restartApp();
  await poll(replacement, 'pending');
  await fixtureSql(`UPDATE cookie_oauth_sessions SET expires_at=unixepoch()-1 WHERE ${sessionRow(first)}`);
  const expiryCounts = await providers.counts({ actorId });
  await poll(first, 'expired');
  check((await providers.counts({ actorId })).poll === expiryCounts.poll, 'local session expiry precedes provider');
  // Clear only this actor's abandoned UI/lifecycle sessions, never fabricate Ready.
  await fixtureSql(`DELETE FROM cookie_oauth_sessions WHERE user_id=${actorId}`);
  for (let i = 0; i < 8; i++) await start();
  const capacityCounts = await providers.counts({ actorId });
  await expect(`${BASE}/yttv/start`, 'POST', undefined, 429);
  check((await providers.counts({ actorId })).start === capacityCounts.start, 'session capacity checked before provider');
  await fixtureSql(`DELETE FROM cookie_oauth_sessions WHERE user_id=${actorId}`);

  // Fill the default 128-domain limit via public paths, then obtain a genuine
  // provider Ready that must persist for a capacity retry across process restart.
  const ownRows = await expect(BASE, 'GET', undefined, 200);
  for (const row of ownRows.filter(r => r.user_id === actorId && !r.is_pool)) await expect(`${BASE}/by-id/${row.id}`, 'DELETE', undefined, 204);
  for (let batch = 0; batch < 2; batch++) {
    const token = await issue();
    const cookies = Object.fromEntries(Array.from({ length: 64 }, (_, i) => {
      const domain = `lifecycle-capacity-${batch * 64 + i}.test`;
      return [domain, [cookie(domain)]];
    }));
    await submit({ token, cookies });
  }
  const retryToken = await issue();
  const full = await owned();
  await submit({ token: retryToken, cookies: { 'lifecycle-capacity-1.test': [cookie('lifecycle-capacity-1.test', 'must-rollback')], 'youtube.com': [cookie('youtube.com')] } }, 409);
  check(await owned() === full, 'sync capacity rollback is atomic');
  check(await fixtureSql(`SELECT count(*) FROM cookie_web_tokens WHERE ${tokenRow(retryToken)}`) === '1', 'capacity failure preserves sync token');
  await providers.set({ actorId, oauth: 'ok', account: 'valid' });
  const ready = await start();
  const readyCounts = await providers.counts({ actorId });
  await poll(ready, null, jwt, 409);
  check((await providers.counts({ actorId })).poll === readyCounts.poll + 1, 'Ready obtained through actual provider poll');
  check(await fixtureSql(`SELECT count(*) FROM cookie_oauth_sessions WHERE ${sessionRow(ready)} AND ready_content IS NOT NULL`) === '1', 'provider Ready durably staged');
  check(await owned() === full, 'OAuth capacity leaves cookies untouched');
  await restartApp();
  await providers.set({ actorId, oauth: 'error', account: 'valid' });
  await expect(`${BASE}/lifecycle-capacity-0.test`, 'DELETE', undefined, 204);
  await submit(body(retryToken, 'lifecycle-capacity-0.test'));
  await expect(`${BASE}/lifecycle-capacity-0.test`, 'DELETE', undefined, 204);
  const retryCounts = await providers.counts({ actorId });
  await poll(ready, 'ok');
  check((await providers.counts({ actorId })).poll === retryCounts.poll, 'restart uses genuine staged Ready without another provider call');
  check(await fixtureSql(`SELECT count(*) FROM cookies WHERE user_id=${actorId} AND domain='youtube.com' AND substr(content,1,10)='__oauth2__'`) === '1', 'OAuth credential persisted');
  const completed = await owned();
  await restartApp();
  check(await owned() === completed, 'completed OAuth survives restart');
  await poll(ready, 'expired');
  check(await owned() === completed, 'completed session replay cannot mutate');
  return { sync: ['independent', 'actor binding', 'size', 'expiry', 'capacity', 'restart', 'concurrent single-use'], oauth: ['ownership', 'independent replacement', 'local expiry', 'session capacity', 'provider Ready capacity retry', 'durable restart', 'completed replay'] };
}
