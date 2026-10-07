import assert from 'node:assert/strict';
import { literal } from './helpers.mjs';

export async function routing(c) {
  for (const settings of [{ inbox: false }, { auth: false }]) {
    await c.launch(settings); await c.ready();
    await c.request('/api/v1/inbox/items', { expected: 404 });
    await c.stop();
  }
  const release = c.bot.holdNextMenu();
  try {
    await c.launch();
    await c.waitFor(async () => { try { return (await fetch(`http://127.0.0.1:${c.httpPort}/health/ready`)).status === 503; } catch { return false; } }, 'diagnostics before preparation');
  } finally { release(); }
  await c.ready();
  await c.request('/api/v1/inbox/items', { expected: 401 });
}

export async function httpContracts(c) {
  const token = await c.login(42), other = await c.login(43);
  const request = (path, options = {}) => c.request('/api/v1/inbox' + path, { token, ...options });
  for (const body of [{ url: 'file:///etc/passwd' }, { url: 'not-a-url' }]) await request('/items', { method: 'POST', body, expected: 422 });
  const item = await request('/items', { method: 'POST', body: { url: 'https://github.com/fixture/http' }, expected: 201 });
  assert(Number.isSafeInteger(item.id));
  await c.waitFor(async () => (await request(`/items/${item.id}`)).status === 'classified', 'real Content and Insight classification');
  assert(c.trusted.calls.some(call => call.path === '/repos/fixture/http/readme'));
  assert(c.trusted.calls.some(call => call.path === '/v1/chat/completions'));
  await c.request(`/api/v1/inbox/items/${item.id}`, { token: other, expected: 404 });
  await c.db.batch('UPDATE users SET ban_until=unixepoch()*1000000+600000000 WHERE id=42');
  try { await request('/items', { expected: 403 }); } finally { await c.db.batch('UPDATE users SET ban_until=NULL WHERE id=42'); }
  // Current feature grant with an unchanged session, not a fabricated JWT.
  await c.db.batch("UPDATE users SET role='restricted' WHERE id=43; INSERT INTO user_permissions(user_id,plugin,feature,granted_by,granted_at) VALUES(43,'inbox','ingest',42,0)");
  await c.request('/api/v1/inbox/items', { token: other });
  await c.db.batch("DELETE FROM user_permissions WHERE user_id=43 AND plugin='inbox' AND feature='ingest'");
  await c.request('/api/v1/inbox/items', { token: other, expected: 403 });
  await c.db.batch("UPDATE users SET role='user' WHERE id=43");
  await request('/items', { method: 'POST', body: { url: 'https://github.com/fixture/page-two' }, expected: 201 });
  const page = await request('/items?limit=1');
  assert.equal(page.items.length, 1); assert(page.next_cursor);
  const next = await request(`/items?limit=1&cursor=${encodeURIComponent(page.next_cursor)}`);
  assert.equal(next.items.length, 1); assert.notEqual(next.items[0].id, page.items[0].id);
  await request('/items?limit=0', { expected: 422 }); await request('/items?cursor=invalid', { expected: 422 });
  const folder = await request('/folders', { method: 'POST', body: { name: 'Local fixture' }, expected: 201 });
  assert.equal((await request(`/folders/${folder.id}`)).id, folder.id);
  await c.request(`/api/v1/inbox/folders/${folder.id}`, { token: other, expected: 404 });
  await c.db.batch("INSERT INTO insight_user_settings(user_id,github_token,github_token_public_repo) VALUES(42,'fixture-read','fixture-write') ON CONFLICT(user_id) DO UPDATE SET github_token=excluded.github_token,github_token_public_repo=excluded.github_token_public_repo");
  await request('/gh_stars/sync', { method: 'POST', expected: 202 });
  const star = await c.waitFor(async () => (await request('/gh_stars')).items?.[0], 'real stars sync');
  assert.notEqual(star.id, 991, 'wire ID is local row ID, not GitHub databaseId');
  await request(`/folders/${folder.id}/stars?star_id=${star.id}`, { method: 'POST', expected: 201 });
  assert.equal((await request(`/gh_stars?folder_id=${folder.id}`)).items[0].id, star.id);
  await request(`/folders/${folder.id}/stars/${star.id}`, { method: 'DELETE', expected: 204 });
  await request(`/gh_stars/${star.id}/star`, { method: 'PUT', expected: 204 });
  assert(c.trusted.calls.some(call => call.method === 'PUT' && call.path === '/user/starred/fixture/repo'));
  await request('/gh_stars/991/star', { method: 'PUT', expected: 404 });
  const body = { name: 'Remote fixture', is_local: false };
  await request('/folders', { method: 'POST', body, expected: 422 });
  const headers = { 'Idempotency-Key': 'fixture-create-1' };
  const remote = await request('/folders', { method: 'POST', body, headers, expected: 201 });
  const writes = () => c.trusted.calls.filter(call => call.body?.query?.includes('createUserList')).length;
  const before = writes();
  assert.equal((await request('/folders', { method: 'POST', body, headers, expected: 201 })).id, remote.id);
  assert.equal(writes(), before);
  await request('/folders', { method: 'POST', body: { ...body, name: 'Conflicting' }, headers, expected: 422 });
  c.trusted.dropCreate = true;
  await request('/folders', { method: 'POST', body: { name: 'Lost receipt', is_local: false }, headers: { 'Idempotency-Key': 'fixture-create-unknown' }, expected: 409 });
  const active = await request('/gh_mutations');
  const unknown = active.find(row => row.state === 'unknown'); assert(unknown);
  const candidates = await request(`/gh_mutations/${unknown.id}/create-candidates`);
  assert.equal(candidates.length, 1);
  await request(`/gh_mutations/${unknown.id}/cancel`, { method: 'POST', expected: 409 });
  const count = writes();
  await c.stop(); await c.launch(); await c.ready();
  await request(`/gh_mutations/${unknown.id}/reconcile`, { method: 'POST', expected: 409 });
  assert.equal(writes(), count, 'unknown create must never resend');
  assert.equal((await request(`/gh_mutations/${unknown.id}/adopt-create`, { method: 'POST', body: { remote_id: candidates[0].id } })).state, 'applied');
  assert.equal(await c.db.query(`SELECT state FROM inbox_gh_mutations WHERE id=${literal(unknown.id)}`), 'applied');
}
