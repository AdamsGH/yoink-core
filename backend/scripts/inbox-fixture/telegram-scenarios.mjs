import assert from 'node:assert/strict';
import { literal } from './helpers.mjs';
import { liveWorkerShutdown } from './live-worker-scenario.mjs';

export async function telegramContracts(c) {
  let message = 20000;
  const saved = async (name, options = {}) => {
    const url = `https://github.com/fixture/${name}`;
    const start = c.bot.calls.length, messageId = message++;
    c.bot.enqueue(42, 42, `/save ${url}`, messageId, 'en', options);
    const reply = await c.waitFor(() => c.bot.calls.slice(start).find(call => call.method === 'sendmessage' && Number(call.data.chat_id) === 42), 'actual /save reply');
    const item = Number(await c.db.query(`SELECT id FROM inbox_items WHERE user_id=42 AND url=${literal(url)}`));
    assert(item > 0);
    return { url, start, messageId, reply, item };
  };
  const creator = await saved('telegram');
  await c.waitFor(async () => (await c.db.query(`SELECT status FROM inbox_items WHERE id=${creator.item}`)) === 'classified', 'Telegram real enrichment/classification');
  const edit = await c.waitFor(() => c.bot.calls.slice(creator.start).find(call => call.method === 'editmessagetext' && Number(call.data.message_id) === creator.reply.messageId), 'completion edits proven bot reply');
  assert.notEqual(Number(edit.data.message_id), creator.messageId);
  assert.equal(await c.db.query(`SELECT tg_reply_message_id FROM inbox_items WHERE id=${creator.item}`), String(creator.reply.messageId));
  await c.waitFor(async () => (await c.db.query(`SELECT state FROM inbox_notifications WHERE item_id=${creator.item}`)) === 'acked', 'durable completion receipt');
  const count = c.bot.calls.filter(call => call.method === 'sendmessage').length;
  c.bot.enqueue(42, 42, `/save ${creator.url}`, creator.messageId);
  // Barrier through later real command, not a sleep-only negative assertion.
  const duplicate = await saved('telegram');
  assert.notEqual(duplicate.reply.messageId, creator.reply.messageId);
  assert.equal(duplicate.item, creator.item);
  assert.equal(await c.db.query(`SELECT tg_reply_message_id FROM inbox_items WHERE id=${creator.item}`), String(creator.reply.messageId));
  assert.equal(c.bot.calls.filter(call => call.method === 'sendmessage').length, count + 1, 'replay sends nothing; distinct duplicate acknowledges once');
  assert.equal(await c.db.query(`SELECT count(*) FROM inbox_jobs WHERE item_id=${creator.item} AND kind='enrich'`), '1');

  c.proxy.dropSend = true;
  const unknown = await saved('unknown-send');
  await c.waitFor(async () => Number(await c.db.query("SELECT count(*) FROM runtime_text_effects WHERE state='unknown'")) > 0, 'lost send receipt durable unknown');
  const sends = c.bot.calls.filter(call => call.method === 'sendmessage').length;
  await c.stop(); await c.launch(); await c.ready();
  c.bot.enqueue(42, 42, `/save ${unknown.url}`, unknown.messageId);
  await saved('send-barrier');
  assert.equal(c.bot.calls.filter(call => call.method === 'sendmessage').length, sends + 1, 'unknown send is not retried on replay/restart');
  assert.equal(await c.db.query(`SELECT coalesce(tg_reply_message_id,0) FROM inbox_items WHERE id=${unknown.item}`), '0', 'lost receipt cannot become completion provenance');

  await c.waitFor(async () => (await c.db.query("SELECT count(*) FROM inbox_jobs WHERE kind IN ('enrich','classify') AND state IN ('ready','running')")) === '0', 'drain earlier enrichment before targeted uncertain edit');
  await c.waitFor(async () => (await c.db.query("SELECT count(*) FROM inbox_notifications WHERE state IN ('ready','claimed','sending')")) === '0', 'drain earlier edits before dropping next receipt');
  c.proxy.dropEdit = true;
  const unknownEdit = await saved('unknown-edit');
  await c.waitFor(async () => (await c.db.query(`SELECT state FROM inbox_notifications WHERE item_id=${unknownEdit.item}`)) === 'unknown', 'lost edit receipt durable unknown');
  const edits = () => c.bot.calls.filter(call => call.method === 'editmessagetext' && Number(call.data.message_id) === unknownEdit.reply.messageId).length;
  assert.equal(edits(), 1);
  await c.stop(); await c.launch(); await c.ready(); const barrier = await saved('edit-barrier');
  await c.waitFor(async () => (await c.db.query(`SELECT state FROM inbox_notifications WHERE item_id=${barrier.item}`)) === 'acked', 'restart notification dispatch barrier');
  assert.equal(edits(), 1, 'uncertain completion edit must never retry');

  // Proven receipts recover without sending or requiring current grants. Simulate
  // only the crash gap after confirmed persistence, never manufacture evidence.
  await c.stop();
  await c.db.batch(`UPDATE runtime_text_effects SET bound=0 WHERE state='confirmed' AND receipt=${creator.reply.messageId}; UPDATE inbox_items SET tg_chat_id=NULL,tg_reply_message_id=NULL WHERE id=${creator.item};`);
  const beforeRecovery = c.bot.calls.length;
  await c.launch(); await c.ready();
  await c.waitFor(async () => (await c.db.query(`SELECT tg_reply_message_id FROM inbox_items WHERE id=${creator.item}`)) === String(creator.reply.messageId), 'restart binds actual confirmed receipt');
  assert.equal(c.bot.calls.slice(beforeRecovery).filter(call => ['sendmessage', 'editmessagetext'].includes(call.method) && Number(call.data.message_id ?? call.messageId) === creator.reply.messageId).length, 0);
  c.bot.enqueue(42, -1004200, '/save https://github.com/fixture/disabled-group', message++, 'en', { thread: 77 });
  await saved('group-barrier');
  assert.equal(await c.db.query("SELECT count(*) FROM inbox_items WHERE url='https://github.com/fixture/disabled-group'"), '0', 'disabled group/topic authority cannot admit Inbox work');
  assert.equal(c.bot.error, undefined);
}

export async function shutdownContracts(c) {
  await c.db.batch("INSERT INTO insight_user_settings(user_id,github_token) VALUES(42,'fixture-read') ON CONFLICT(user_id) DO UPDATE SET github_token=excluded.github_token");
  const token = await c.login(42);
  c.trusted.holdRepository = true;
  const heldBefore = c.trusted.held;
  const item = await c.request('/api/v1/inbox/items', { token, method: 'POST', body: { url: 'https://github.com/fixture/shutdown' }, expected: 201 });
  await c.waitFor(() => c.trusted.held > heldBefore, 'admitted real Github read');
  assert.equal(await c.db.query(`SELECT state FROM inbox_jobs WHERE item_id=${item.id} AND kind='enrich'`), 'running');
  await c.stop(); await c.workspaceClean();
  assert.equal(await c.db.query(`SELECT state,attempts,coalesce(lease_token,'') FROM inbox_jobs WHERE item_id=${item.id} AND kind='enrich'`), 'ready|0|', 'owning shutdown restores attempt and releases fenced lease');
  c.trusted.holdRepository = false;
  await c.launch(); await c.ready();
  await c.waitFor(async () => (await c.db.query(`SELECT status FROM inbox_items WHERE id=${item.id}`)) === 'classified', 'restart completes admitted item');
  const denied = await c.request('/api/v1/inbox/items', { token, method: 'POST', body: { url: c.trusted.origin + '/untrusted-private' }, expected: 201 });
  await c.waitFor(async () => (await c.db.query(`SELECT state FROM inbox_jobs WHERE item_id=${denied.id} AND kind='enrich'`)) === 'failed', 'actual confined worker terminal policy denial', 150000);
  assert.equal(c.trusted.calls.filter(call => call.path === '/untrusted-private').length, 0, 'untrusted worker cannot reach loopback');
  assert.equal(await c.db.query(`SELECT attempts,coalesce(lease_token,'') FROM inbox_jobs WHERE item_id=${denied.id} AND kind='enrich'`), '1|', 'policy denial is terminal, not a repeatable external failure');
  await c.workspaceClean();
  c.trusted.holdStars = true;
  await c.request('/api/v1/inbox/gh_stars/sync', { token, method: 'POST', expected: 202 });
  const job = await c.db.query("SELECT max(id) FROM inbox_jobs WHERE kind='stars_sync'");
  await c.waitFor(async () => (await c.db.query(`SELECT state FROM inbox_jobs WHERE id=${job}`)) === 'failed', 'real bounded read timeouts consume configured retry budget', 60000);
  assert.equal(await c.db.query(`SELECT attempts,coalesce(lease_token,'') FROM inbox_jobs WHERE id=${job}`), '2|');
  c.trusted.holdStars = false;
  await c.request('/api/v1/inbox/gh_stars/sync', { token, method: 'POST', expected: 202 });
  await c.waitFor(async () => (await c.db.query(`SELECT state FROM inbox_jobs WHERE kind='stars_sync' AND id>${job} ORDER BY id DESC LIMIT 1`)) === 'done', 'terminal retry budget does not stall later eligible work');
  await liveWorkerShutdown(c);
}
