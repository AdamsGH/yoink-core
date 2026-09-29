import assert from 'node:assert/strict';
import { setTimeout as sleep } from 'node:timers/promises';
import { commandHarness } from './command-harness.mjs';

export async function runCookieScenarios(context) {
  const { botApi, fixtureSql, waitFor, restartApp, sourceFixture, sourcePort, cookieSecret, queryJob, assertWorkspaceClean } = context;
  const h = commandHarness(context, 72000);
  const literal = value => value === null ? 'NULL' : typeof value === 'number' ? String(value) : `'${value.replaceAll("'", "''")}'`;
  const columns = ['id', 'user_id', 'domain', 'content', 'is_valid', 'created_at', 'updated_at', 'validated_at', 'is_pool', 'label', 'avatar_url', 'content_hash', 'session_key'];
  const savedCookies = await fixtureSql('SELECT * FROM cookies ORDER BY id');
  const savedPersonal = JSON.parse(await fixtureSql(`SELECT json_group_array(json_object(${columns.flatMap(name => [literal(name), name]).join(',')})) FROM (SELECT * FROM cookies WHERE user_id=42 AND is_pool=0 ORDER BY id)`));
  const savedUser = JSON.parse(await fixtureSql("SELECT json_object('role',role,'ban_until',ban_until) FROM users WHERE id=42"));
  const savedSettings = await fixtureSql('SELECT * FROM dl_user_settings WHERE user_id=42');
  const settings = JSON.parse(await fixtureSql("SELECT json_object('proxy_enabled',proxy_enabled,'quality',quality,'use_pool_cookies',use_pool_cookies,'updated_at',updated_at) FROM dl_user_settings WHERE user_id=42") || 'null');
  const cookieJar = host => `# Netscape HTTP Cookie File\n${host}\tFALSE\t/\tFALSE\t${Math.floor(Date.now() / 1000) + 86400}\tfixture_auth\t${cookieSecret}\n`;
  let next = 70000;
  const extraIds = [];
  async function insert(domain, { actor = 42, pool = false, valid = true, content = cookieJar(domain) } = {}) {
    const id = Number(await fixtureSql(`INSERT INTO cookies(user_id,domain,content,is_valid,is_pool,created_at,updated_at) VALUES(${actor},${literal(domain)},${literal(content)},${Number(valid)},${Number(pool)},unixepoch('now')*1000000,unixepoch('now')*1000000) RETURNING id`));
    assert(Number.isSafeInteger(id) && id > 0);
    if (actor !== 42 || pool) extraIds.push(id);
    return id;
  }
  async function command(text = '/cookie', { messageId = next++, language = 'en' } = {}) {
    const start = botApi.calls.length;
    botApi.enqueue(42, 42, text, messageId, language);
    const reply = await waitFor(() => botApi.calls.slice(start).find(c => c.method === 'sendmessage' && Number(c.data.chat_id) === 42), 'personal cookie command response');
    assert(!reply.data.text.includes(cookieSecret));
    assert(!reply.data.text.includes('[telegram.'), 'cookie response must resolve its exact i18n parameters');
    return { ...reply, incomingMessageId: messageId };
  }
  const buttons = call => call.data.reply_markup?.inline_keyboard?.flat() ?? [];
  async function click(menu, data, { edit = false, actor = 42, ...options } = {}) {
    const start = botApi.calls.length;
    const id = botApi.enqueueCallback(actor, 42, menu.messageId, data, options);
    const ack = await waitFor(() => botApi.calls.slice(start).find(c => c.method === 'answercallbackquery' && c.data.callback_query_id === id), 'personal cookie callback acknowledgement');
    const changed = edit ? await waitFor(() => botApi.calls.slice(start).find(c => c.method === 'editmessagetext' && c.data.message_id === menu.messageId), 'personal cookie page edit') : undefined;
    if (!edit) assert(!botApi.calls.slice(start).some(c => c.method === 'editmessagetext'));
    return { ack, changed };
  }
  const personalCount = () => fixtureSql('SELECT count(*) FROM cookies WHERE user_id=42 AND is_pool=0');
  const untouched = () => fixtureSql('SELECT * FROM cookies WHERE user_id<>42 OR is_pool=1 ORDER BY id');
  try {
    // Only the disposable fixture DB is rearranged. Restore imported personal rows exactly below.
    await fixtureSql('DELETE FROM cookies WHERE user_id=42 AND is_pool=0');
    const initialOperations = await fixtureSql('SELECT count(*) FROM runtime_operations');
    if (settings) await fixtureSql("UPDATE dl_user_settings SET quality='broken-quality' WHERE user_id=42");
    const empty = await command();
    assert.match(empty.data.text, /no.*cookie|cookie.*empty/i);
    for (let i = 0; i < 11; i++) await insert(`p${String(i).padStart(2, '0')}.example`);
    await insert('broken.example', { content: 'not a Netscape jar' });
    const large = await insert('large.example');
    await fixtureSql(`UPDATE cookies SET content=hex(zeroblob(262145)) WHERE id=${large}`);
    await insert(`https://name:${cookieSecret}@redaction.example/file`, { content: cookieJar('redaction.example') });
    await insert('invalid.example', { valid: false });
    await insert('p00.example', { pool: true });
    await insert('p00.example', { actor: 8000000000011 });
    const otherRows = await untouched();
    const menu = await command('/cookie@YOINK_FIXTURE_BOT');
    assert.match(menu.data.text, /p00\.example/);
    assert(!menu.data.text.includes('p10.example'));
    const nextPage = buttons(menu).find(b => b.callback_data?.startsWith('cookie:page:'));
    assert(nextPage, 'personal list must offer bounded pagination');
    const second = (await click(menu, nextPage.callback_data, { edit: true })).changed;
    assert.match(second.data.text, /p10\.example/);
    assert(!second.data.text.includes('https://') && !second.data.text.includes(cookieSecret), 'malformed stored domain must not reveal URL/userinfo');
    assert(!buttons(second).some(b => b.callback_data?.startsWith('cookie:page:')));
    const status = await command('/cookie .P00.EXAMPLE');
    assert.match(status.data.text, /\b1\b/);
    for (const domain of ['broken.example', 'large.example', 'invalid.example']) {
      assert.match((await command(`/cookie ${domain}`)).data.text, /invalid|unusable|malformed/i);
    }
    assert.match((await command('/cookie absent.example')).data.text, /no.*cookie|not.*found/i);
    for (const text of ['/cookie clear extra', '/cookie del', '/cookie del p00.example extra', '/cookie token', `/cookie https://name:${cookieSecret}@redaction.example/file`]) {
      const reply = await command(text);
      assert.match(reply.data.text, /\/cookie/);
      assert.equal(buttons(reply).length, 0);
    }
    for (const data of ['cookie:page:not-a-number', 'cookie:page:1:extra', 'cookie:clear', 'cookie:page:' + '9'.repeat(80)]) {
      assert.equal((await click(menu, data)).ack.data.show_alert, true);
    }
    for (const options of [{ actor: 99 }, { isBot: true }, { inaccessible: true },
      { messagePatch: { message_thread_id: 9 } },
      { messagePatch: { chat: { id: -42, type: 'group', title: 'Fixture' } } },
      { messagePatch: { from: { id: 999, is_bot: true, first_name: 'Foreign bot' } } }]) {
      await click(menu, nextPage.callback_data, options);
    }
    const beforeSilent = botApi.calls.filter(c => c.method === 'sendmessage').length;
    for (const [actor, chat, text] of [[99, 99, '/cookie clear'], [42, -10042, '/cookie clear'], [42, 42, '/cookie@other_bot clear']]) botApi.enqueue(actor, chat, text, next++);
    await command('/help');
    await sleep(300);
    assert.equal(botApi.calls.filter(c => c.method === 'sendmessage').length, beforeSilent + 1);
    const beforeDenied = await personalCount();
    await fixtureSql("UPDATE users SET ban_until=unixepoch('now')*1000000+600000000 WHERE id=42");
    assert.equal((await command('/cookie clear')).data.text, 'This operation is not allowed.');
    assert.equal((await click(menu, nextPage.callback_data)).ack.data.show_alert, true);
    assert.equal(await personalCount(), beforeDenied);
    await fixtureSql(`UPDATE users SET ban_until=${literal(savedUser.ban_until)} WHERE id=42`);
    const deleted = await command('/cookie del .P00.EXAMPLE');
    assert.match(deleted.data.text, /\b1\b/);
    assert.equal(await personalCount(), String(Number(beforeDenied) - 1));
    assert.equal(await untouched(), otherRows, 'domain removal must preserve foreign personal and pool rows');
    await insert('p00.example');
    await restartApp();
    for (const text of ['/cookie del p00.example', '/cookie clear']) {
      assert.match((await command(text, { messageId: deleted.incomingMessageId })).data.text, /already|processed/i);
    }
    await command('/cookie clear', { messageId: deleted.incomingMessageId - 1 });
    assert.equal(await personalCount(), beforeDenied, 'old updates after restart must not remove replacement cookies');
    const cleared = await command('/cookie clear', { language: 'ru' });
    assert.match(cleared.data.text, /удал|очищ/i);
    assert.equal(await personalCount(), '0', 'clear removes valid and invalid personal rows');
    assert.equal(await untouched(), otherRows);
    await insert('new.example');
    await command('/cookie clear', { messageId: cleared.incomingMessageId });
    assert.equal(await personalCount(), '1');
    await command('/cookie clear');
    const noMatch = await command('/cookie del no-match.example');
    await insert('no-match.example');
    await command('/cookie clear', { messageId: noMatch.incomingMessageId });
    assert.equal(await personalCount(), '1', 'no-match deletion must still fence later replay');
    await command('/cookie clear');
    assert.equal(await fixtureSql('SELECT count(*) FROM runtime_operations'), initialOperations, 'cookie controls never admit jobs');
    if (settings) {
      assert.equal(await fixtureSql('SELECT quality FROM dl_user_settings WHERE user_id=42'), 'broken-quality');
      await fixtureSql(`UPDATE dl_user_settings SET quality=${literal(settings.quality)} WHERE user_id=42`);
    }
    assert.equal(await fixtureSql('SELECT * FROM dl_user_settings WHERE user_id=42'), savedSettings);

    // A real selected credential is revoked through the public command, with a valid pool candidate left behind.
    if (!settings) await command('/settings quality best');
    await fixtureSql('UPDATE dl_user_settings SET proxy_enabled=0,use_pool_cookies=1 WHERE user_id=42');
    const poolId = await insert('127.0.0.1', { pool: true });
    const personalId = await insert('127.0.0.1');
    const video = url => `/video --height best --video-codec h264 --container mp4 --send-as-file false ${url}`;
    const url = `http://127.0.0.1:${sourcePort}/cookie-media?case=cookie-controls`;
    const delivered = await h.execute('personal cookie before removal', video(url), 'sendvideo',
      { video: true, audio: true, durationMin: 0.8, durationMax: 1.5 }, { filename: 'media.mp4', mimeType: 'video/mp4' });
    assert.equal((await h.request(delivered.operationId)).network.cookie.id, personalId);
    const originalOperation = await fixtureSql(`SELECT * FROM runtime_operations WHERE id='${delivered.operationId}'`);
    const originalReceipt = await fixtureSql(`SELECT * FROM runtime_delivery_units WHERE operation_id='${delivered.operationId}' ORDER BY attempt,ordinal`);
    const cached = await fixtureSql(`SELECT * FROM runtime_media_cache WHERE cache_key=(SELECT cache_key FROM runtime_operations WHERE id='${delivered.operationId}')`);
    sourceFixture.closeGate();
    const sends = botApi.sendCalls;
    const blocker = await h.submit(video(`http://127.0.0.1:${sourcePort}/gated?case=cookie-controls`));
    await waitFor(() => sourceFixture.gatedResponses.size > 0, 'cookie-backed worker reached source');
    const queued = await h.submit(video(`${url}&queued=1`));
    assert((await queryJob(queued.operationId)).startsWith('queued|'));
    const frozen = await h.request(queued.operationId);
    assert.equal(frozen.network.cookie.id, personalId);
    const sourceRequests = sourceFixture.protectedRequests;
    await command('/cookie del 127.0.0.1');
    assert.deepEqual(await h.request(queued.operationId), frozen);
    sourceFixture.releaseGatedResponses();
    await waitFor(async () => (await queryJob(blocker.operationId)).startsWith('failed|') && (await queryJob(queued.operationId)).startsWith('failed|'), 'cookie removal invalidates running and queued jobs');
    assert.equal(sourceFixture.protectedRequests, sourceRequests, 'queued job must not reselect pool credentials');
    assert.equal(botApi.sendCalls, sends, 'removed credential cannot authorize delivery');
    assert.equal(await fixtureSql(`SELECT count(*) FROM cookies WHERE id=${poolId}`), '1');
    assert.equal(await fixtureSql(`SELECT * FROM runtime_operations WHERE id='${delivered.operationId}'`), originalOperation);
    assert.equal(await fixtureSql(`SELECT * FROM runtime_delivery_units WHERE operation_id='${delivered.operationId}' ORDER BY attempt,ordinal`), originalReceipt);
    assert.equal(await fixtureSql(`SELECT * FROM runtime_media_cache WHERE cache_key=(SELECT cache_key FROM runtime_operations WHERE id='${delivered.operationId}')`), cached);
    const pooled = await h.execute('permitted pool cookie for a new admission', video(`${url}&pool=1`), 'sendvideo',
      { video: true, audio: true, durationMin: 0.8, durationMax: 1.5 }, { filename: 'media.mp4', mimeType: 'video/mp4' });
    assert.equal((await h.request(pooled.operationId)).network.cookie.id, poolId);
    assert.equal(await fixtureSql(`SELECT * FROM cookies WHERE (user_id<>42 OR is_pool=1) AND id<>${poolId} ORDER BY id`), otherRows);
    assert(!JSON.stringify(botApi.calls).includes(cookieSecret));
    assert.equal(botApi.error, undefined);
    await assertWorkspaceClean();
  } finally {
    sourceFixture.releaseGatedResponses();
    botApi.setExpectedUpload(null);
    await fixtureSql(`UPDATE users SET role=${literal(savedUser.role)},ban_until=${literal(savedUser.ban_until)} WHERE id=42`);
    await fixtureSql('DELETE FROM cookies WHERE user_id=42 AND is_pool=0');
    if (extraIds.length) await fixtureSql(`DELETE FROM cookies WHERE id IN (${extraIds.join(',')})`);
    for (const row of savedPersonal) await fixtureSql(`INSERT INTO cookies(${columns.join(',')}) VALUES(${columns.map(name => literal(row[name])).join(',')})`);
    if (settings) await fixtureSql(`UPDATE dl_user_settings SET ${Object.entries(settings).map(([name, value]) => `${name}=${literal(value)}`).join(',')} WHERE user_id=42`);
    else await fixtureSql('DELETE FROM dl_user_settings WHERE user_id=42');
    assert.equal(await fixtureSql('SELECT * FROM cookies ORDER BY id'), savedCookies, 'restore every original cookie row exactly');
    assert.equal(await fixtureSql('SELECT * FROM dl_user_settings WHERE user_id=42'), savedSettings);
  }
  return 'private personal-cookie list/status/pagination/delete/clear, pool and user isolation, secret-free invalid records, authorization, durable replay fencing, EN/RU, real cookie-backed delivery and frozen-reference revocation without pool reselection passed';
}
