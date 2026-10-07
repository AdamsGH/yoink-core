import assert from 'node:assert/strict';

export async function groupContracts(c) {
  await c.stop(); await c.launch({ groups: true }); await c.ready();
  const token = await c.login(42), group = -1004300;
  let date = Math.floor(Date.now() / 1000), message = 30000;
  const request = (path, options = {}) => c.request('/api/v1' + path, { token, ...options });
  async function observe(subject, status, bot = false) {
    const update = c.bot.enqueueMembership(group, subject, status, { bot, date: ++date });
    await c.waitFor(() => c.bot.polls.some(poll => poll.offset > update), 'actual membership update consumed');
  }
  async function denied(url, thread) {
    const start = c.bot.calls.length;
    c.bot.enqueue(43, group, `/save ${url}`, message++, 'en', { thread });
    c.bot.enqueue(42, 42, '/save', message++);
    await c.waitFor(() => c.bot.calls.slice(start).some(call => call.method === 'sendmessage' && Number(call.data.chat_id) === 42), 'private command authority barrier');
    assert.equal(await c.db.query(`SELECT count(*) FROM inbox_items WHERE user_id=43 AND url='${url}'`), '0');
  }
  await observe(123456, 'member', true); await observe(43, 'member');
  await request(`/groups/${group}`, { method: 'PATCH', body: { enabled: true, auto_grant_role: 'user', allow_pm: true } });
  const topic = await request(`/groups/${group}/threads`, { method: 'POST', body: { thread_id: 18, name: 'Inbox fixture topic', enabled: true }, expected: 201 });
  await request(`/groups/${group}/threads`, { method: 'POST', body: { thread_id: 19, enabled: false }, expected: 201 });
  await denied('https://github.com/fixture/topic-denied', 19);
  await c.waitFor(async () => (await c.db.query("SELECT count(*) FROM inbox_jobs WHERE state IN ('running','ready')")) === '0', 'earlier work drained before authority gate');
  await c.db.batch("INSERT INTO user_permissions(user_id,plugin,feature,granted_by,granted_at) VALUES(43,'inbox','classify',42,0)");
  c.trusted.holdRepository = true;
  const heldBefore = c.trusted.held;
  try {
    const start = c.bot.calls.length;
    c.bot.enqueue(43, group, '/save https://github.com/fixture/group-held', message++, 'en', { thread: 18 });
    const reply = await c.waitFor(() => c.bot.calls.slice(start).find(call => call.method === 'sendmessage' && Number(call.data.chat_id) === group), 'group/topic save actual receipt');
    assert.equal(reply.data.message_thread_id, 18);
    const item = Number(await c.db.query("SELECT id FROM inbox_items WHERE user_id=43 AND url='https://github.com/fixture/group-held'"));
    await c.waitFor(() => c.trusted.held > heldBefore, 'enrichment admitted before topic revocation');
    await request(`/groups/${group}/threads`, { method: 'POST', body: { thread_id: 18, enabled: false }, expected: 201 });
    // Release by restart: owning shutdown restores admission and closes the held
    // HTTP read. The persisted bot receipt survives independently of fresh grants.
    await c.stop(); c.trusted.holdRepository = false;
    await c.launch({ groups: true }); await c.ready();
    await c.waitFor(async () => (await c.db.query(`SELECT status FROM inbox_items WHERE id=${item}`)) === 'classified', 'group real pipeline after restart');
    await c.waitFor(async () => (await c.db.query(`SELECT state FROM inbox_notifications WHERE item_id=${item}`)) === 'cancelled', 'revoked topic denies completion dispatch');
    assert(!c.bot.calls.slice(start).some(call => call.method === 'editmessagetext' && Number(call.data.chat_id) === group && Number(call.data.message_id) === reply.messageId));
    await request(`/groups/${group}/threads`, { method: 'POST', body: { thread_id: 18, enabled: true }, expected: 201 });
    await observe(43, 'left');
    await denied('https://github.com/fixture/member-left', 18);
    assert(topic.id > 0);
  } finally {
    c.trusted.holdRepository = false;
    await c.db.batch("DELETE FROM user_permissions WHERE user_id=43 AND plugin='inbox' AND feature='classify'");
    await request(`/groups/${group}`, { method: 'DELETE', expected: 204 });
    await c.stop(); await c.launch(); await c.ready();
  }
}
