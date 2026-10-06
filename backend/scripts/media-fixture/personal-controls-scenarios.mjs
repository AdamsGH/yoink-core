import assert from 'node:assert/strict';
import { commandHarness } from './command-harness.mjs';

// Opt-in only: uses the isolated native acceptance DB and fixture transports.
export async function runPersonalControlsScenarios(context) {
  const { botApi, fixtureSql, waitFor, sourceFixture, sourcePort, cookieSecret, proxyFixture, proxyUrl } = context;
  const actor = 43;
  const h = commandHarness(context, 2_500_000);
  const quote = v => v === null ? 'NULL' : typeof v === 'number' ? String(v) : `'${String(v).replaceAll("'", "''")}'`;
  const saved = [];
  for (const [table, where] of [['users', 'id=43'], ['cookies', 'user_id=43 AND is_pool=0'], ['dl_user_settings', 'user_id=43'], ['runtime_personal_cookie_mutation_watermarks', 'actor_id=43']]) {
    const columns = (await fixtureSql(`SELECT name FROM pragma_table_info('${table}') ORDER BY cid`)).trim().split('\n');
    const rows = JSON.parse(await fixtureSql(`SELECT json_group_array(json_object(${columns.flatMap(c => [quote(c), c]).join(',')})) FROM ${table} WHERE ${where}`));
    saved.push({ table, where, columns, rows });
  }
  const ownerCookies = await fixtureSql('SELECT * FROM cookies WHERE user_id<>43 OR is_pool=1 ORDER BY id');
  const ownerSettings = await fixtureSql('SELECT * FROM dl_user_settings WHERE user_id<>43 ORDER BY user_id');
  let next = 2_400_000;
  const jar = Buffer.from(`# Netscape HTTP Cookie File\n127.0.0.1\tFALSE\t/\tFALSE\t4102444800\tfixture_auth\t${cookieSecret}\n`);
  const state = () => fixtureSql('SELECT * FROM cookies WHERE user_id=43 AND is_pool=0 ORDER BY id');
  async function send(text, id = next++) {
    const start = botApi.calls.length;
    botApi.enqueue(actor, actor, text, id);
    const reply = await waitFor(() => botApi.calls.slice(start).find(c => c.method === 'sendmessage' && Number(c.data.chat_id) === actor), 'personal ordinary control reply');
    assert(!reply.data.text.includes(cookieSecret) && !reply.data.text.includes(proxyUrl));
    return reply;
  }
  async function click(menu, data, { from = actor, edit = false, ...options } = {}) {
    const start = botApi.calls.length;
    const id = botApi.enqueueCallback(from, actor, menu.messageId, data, options);
    const ack = await waitFor(() => botApi.calls.slice(start).find(c => c.method === 'answercallbackquery' && c.data.callback_query_id === id), 'personal callback ack');
    if (edit) await waitFor(() => botApi.calls.slice(start).some(c => c.method === 'editmessagetext' && c.data.message_id === menu.messageId), 'personal callback edit');
    else assert(!botApi.calls.slice(start).some(c => c.method === 'editmessagetext' && c.data.message_id === menu.messageId));
    assert(!JSON.stringify(botApi.calls.slice(start)).includes(cookieSecret));
    return ack;
  }
  try {
    await fixtureSql("UPDATE users SET role='user',ban_until=NULL,language='en' WHERE id=43");
    await fixtureSql('DELETE FROM cookies WHERE user_id=43 AND is_pool=0');
    await fixtureSql('DELETE FROM runtime_personal_cookie_mutation_watermarks WHERE actor_id=43');
    const id = 'ordinary-personal-cookie-document';
    botApi.setDocumentFile(id, jar);
    const start = botApi.calls.length;
    const uploadMessage = next++;
    botApi.enqueueDocument(actor, actor, '/cookie 127.0.0.1', uploadMessage, id, { filename: '127.0.0.1.txt' });
    const upload = await waitFor(() => botApi.calls.slice(start).find(c => c.method === 'sendmessage' && Number(c.data.chat_id) === actor), 'ordinary actual file upload');
    assert.match(upload.data.text, /Personal cookies for 127\.0\.0\.1 are stored \(1 entries\)/);
    assert(!upload.data.text.includes(cookieSecret));
    const stored = await state();
    await send('/cookie clear', uploadMessage); // Immutable message replay cannot delete the jar.
    assert((await state()) === stored, "immutable cookie replay");
    assert.match((await send('/cookie 127.0.0.1')).data.text, /1/);
    const importPath = '/cookie-import/ordinary-personal';
    sourceFixture.setCookieImportResponse(importPath, { body: jar, headers: { 'content-type': 'text/plain' } });
    assert.match((await send(`/cookie http://127.0.0.1:${sourcePort}${importPath} 127.0.0.1`)).data.text, /Personal cookies were saved/);
    const menu = await send('/cookie');
    for (const options of [{ from: 42 }, { isBot: true }, { messagePatch: { from: { id: 999, is_bot: true, first_name: 'Wrong' } } }, { messagePatch: { chat: { id: 42, type: 'private', first_name: 'Wrong' } } }, { messagePatch: { sender_chat: { id: -99, type: 'channel', title: 'Wrong' } } }, { messagePatch: { message_thread_id: 9 } }]) {
      await click(menu, 'cookie:page:0', options);
    }
    await send('/settings quality best');
    await fixtureSql(`UPDATE dl_user_settings SET proxy_enabled=0,proxy_url=${quote(proxyUrl)} WHERE user_id=43`);
    const proxy = await send('/proxy');
    const on = proxy.data.reply_markup.inline_keyboard.flat().find(b => b.callback_data?.startsWith('proxy:on:')).callback_data;
    await click(proxy, on, { edit: true });
    assert.equal((await click(proxy, on)).data.show_alert, true); // revision CAS
    const hits = proxyFixture.authHits;
    const result = await h.execute('ordinary owned cookie and proxy extraction', `/video --height best http://127.0.0.1:${sourcePort}/cookie-media?case=ordinary-owned`, 'sendvideo', { video: true, audio: true }, {}, { actor, chat: actor, messageId: next++ });
    const frozen = await h.request(result.operationId);
    assert(frozen.network.cookie && frozen.network.proxy);
    assert(proxyFixture.authHits > hits);
    for (const mutation of ["role='restricted'", "role='user',ban_until=4102444800000000"]) {
      await fixtureSql(`UPDATE users SET ${mutation} WHERE id=43`);
      const before = await state();
      await click(menu, 'cookie:page:0');
      await click(proxy, on);
      await send(`/cookie http://127.0.0.1:${sourcePort}${importPath} denied.example`);
      assert((await state()) === before, "revoked actor cannot mutate cookies");
      assert.deepEqual(await h.request(result.operationId), frozen);
    }
    await fixtureSql("UPDATE users SET role='user',ban_until=NULL WHERE id=43");
    const offMenu = await send('/proxy');
    const off = offMenu.data.reply_markup.inline_keyboard.flat().find(b => b.callback_data?.startsWith('proxy:off:')).callback_data;
    await click(offMenu, off, { edit: true });
    const directHits = proxyFixture.hits;
    const direct = await h.execute('ordinary direct owned cookie extraction', `/video --height best http://127.0.0.1:${sourcePort}/cookie-media?case=ordinary-direct`, 'sendvideo', { video: true, audio: true }, {}, { actor, chat: actor, messageId: next++ });
    assert.equal((await h.request(direct.operationId)).network.proxy, null);
    assert.equal(proxyFixture.hits, directHits);
    await send('/cookie del 127.0.0.1');
    assert.equal(await fixtureSql('SELECT count(*) FROM cookies WHERE user_id=43 AND is_pool=0'), '0');
    assert((await fixtureSql('SELECT * FROM cookies WHERE user_id<>43 OR is_pool=1 ORDER BY id')) === ownerCookies, 'foreign and pool cookies unchanged');
    assert((await fixtureSql('SELECT * FROM dl_user_settings WHERE user_id<>43 ORDER BY user_id')) === ownerSettings, 'foreign settings unchanged');
    return 'ordinary personal controls: file/URL/owned extraction/proxy routing/replay/revocation/fences';
  } finally {
    for (const { table, where, columns, rows } of saved.reverse()) {
      if (table === 'users') {
        for (const row of rows) await fixtureSql(`UPDATE users SET ${columns.filter(c => c !== 'id').map(c => c+'='+quote(row[c])).join(',')} WHERE id=43`);
        continue;
      }
      await fixtureSql(`DELETE FROM ${table} WHERE ${where}`);
      for (const row of rows) await fixtureSql(`INSERT INTO ${table}(${columns.join(',')}) VALUES(${columns.map(c => quote(row[c])).join(',')})`);
    }
  }
}
