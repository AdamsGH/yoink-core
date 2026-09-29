import assert from 'node:assert/strict';
import { commandHarness } from './command-harness.mjs';
import { withRussianPreference } from './language-fixture.mjs';

const options = ['best', '2160', '1440', '1080', '720', '480', '360', '240', '144'];
const literal = value => value === null ? 'NULL' : typeof value === 'number' ? String(value) : `'${String(value).replaceAll("'", "''")}'`;

export async function runQualityScenarios(context) {
  const { botApi, sourcePort, sourceFixture, fixtureSql, waitFor, restartApp, assertWorkspaceClean } = context;
  const h = commandHarness(context, 18000);
  let nextMessage = 18000;
  let nextCallback = 0;
  const sourceUrl = `http://127.0.0.1:${sourcePort}/media?case=durable-quality-private-marker`;
  const settingsFields = ['quality', 'codec', 'container', 'proxy_enabled', 'proxy_url', 'keyboard', 'subs_enabled', 'subs_auto', 'subs_always_ask', 'subs_lang', 'split_size', 'nsfw_blur', 'mediainfo', 'send_as_file', 'audio_codec', 'gallery_zip', 'use_pool_cookies', 'youtube_auth_mode', 'dm_topic_thread_id', 'args_json', 'created_at', 'updated_at'];
  const userFields = ['role', 'ban_until', 'language', 'updated_at'];
  const settingsSql = fields => `SELECT json_object(${fields.map(f => `'${f}',${f}`).join(',')}) FROM dl_user_settings WHERE user_id=42`;
  const oldSettings = await fixtureSql(settingsSql(settingsFields));
  const oldUser = JSON.parse(await fixtureSql(`SELECT json_object(${userFields.map(f => `'${f}',${f}`).join(',')}) FROM users WHERE id=42`));
  const oldSessions = await fixtureSql("SELECT coalesce(json_group_array(json_object('actor_id',actor_id,'bot_id',bot_id,'chat_id',chat_id,'id',id,'revision',revision,'stage',stage,'origin_message_id',origin_message_id,'last_message_id',last_message_id,'request_json',request_json,'selected',selected,'operation_id',operation_id)),'[]') FROM runtime_quality_sessions WHERE actor_id=42");
  const count = () => fixtureSql('SELECT count(*) FROM runtime_operations');
  const buttons = reply => (typeof reply.data.reply_markup === 'string' ? JSON.parse(reply.data.reply_markup) : reply.data.reply_markup)?.inline_keyboard?.flat() ?? [];
  async function message(text, { id = nextMessage++, language = 'en', thread, actor = 42, chat = 42, entities } = {}) {
    const start = botApi.calls.length;
    botApi.enqueue(actor, chat, text, id, language, { thread, entities });
    const reply = await waitFor(() => botApi.calls.slice(start).find(c => c.method === 'sendmessage' && Number(c.data.chat_id) === chat), `quality response to ${text.split(' ')[0]}`);
    assert(!reply.data.text.includes('durable-quality-private-marker'), 'quality prompt must not echo the source URL');
    return { reply, start, id };
  }
  function findButton(reply, wanted) {
    const found = buttons(reply).find(b => b.callback_data?.startsWith(wanted));
    assert(found, `expected an actual served button beginning ${wanted}`);
    assert(Buffer.byteLength(found.callback_data) <= 64, 'callback payload must fit Telegram limit');
    return found.callback_data;
  }
  async function callback(data, origin, { actor = 42, chat = 42, ...flags } = {}) {
    const start = botApi.calls.length;
    const id = `quality-${++nextCallback}`;
    botApi.enqueueCallback(actor, chat, origin.reply.messageId, data, { id, ...flags });
    const ack = await waitFor(() => botApi.calls.slice(start).find(c => c.method === 'answercallbackquery' && c.data.callback_query_id === id), 'quality callback acknowledgement');
    return { start, ack };
  }
  async function admission(from) {
    const reply = await waitFor(() => botApi.calls.slice(from).find(c => c.method === 'sendmessage' && /[0-9a-f]{8}-[0-9a-f-]{27}/i.test(c.data.text)), 'quality operation admission');
    return reply.data.text.match(/[0-9a-f]{8}-[0-9a-f-]{27}/i)[0];
  }
  async function session() {
    return JSON.parse(await fixtureSql("SELECT json_object('id',id,'revision',revision,'stage',stage,'origin_message_id',origin_message_id,'last_message_id',last_message_id,'request_json',request_json,'selected',selected,'operation_id',operation_id) FROM runtime_quality_sessions WHERE actor_id=42 AND bot_id=123456 AND chat_id=42"));
  }
  try {
    await fixtureSql(`INSERT INTO dl_user_settings (user_id, quality, codec, container, proxy_enabled, keyboard, subs_enabled, subs_auto, subs_always_ask, subs_lang, split_size, nsfw_blur, mediainfo, send_as_file, audio_codec, gallery_zip, use_pool_cookies, youtube_auth_mode, args_json, created_at, updated_at) VALUES (42, 'best', 'avc1', 'mp4', 0, '2x3', 0, 0, 0, 'en', 2043000000, 1, 0, 0, 'best', 0, 1, 'cookies', '{}', unixepoch('now')*1000000, unixepoch('now')*1000000) ON CONFLICT(user_id) DO UPDATE SET quality='best',codec='avc1',container='mp4',send_as_file=0,audio_codec='best',subs_lang='en',proxy_enabled=0`);
    await message('/settings quality ask');
    assert.equal(JSON.parse(await fixtureSql(settingsSql(['quality']))).quality, 'ask');
    const settingsMenu = await message('/settings');
    const menuButton = findButton(settingsMenu.reply, 'prefs:quality:');
    assert(menuButton.startsWith('prefs:quality:'));
    const settingsClick = await callback(findButton(settingsMenu.reply, 'prefs:quality:_menu'), settingsMenu);
    const qualitySubmenu = await waitFor(() => botApi.calls.slice(settingsClick.start).find(c => c.method === 'editmessagetext'), 'quality settings submenu');
    assert(qualitySubmenu && buttons({ data: qualitySubmenu.data }).some(b => b.callback_data.includes(':ask:')),
      'the real settings quality menu must offer ask');

    const before = await count();
    const prefsBeforePrompt = await fixtureSql(settingsSql(settingsFields));
    const sourceBefore = sourceFixture.mediaRequests;
    const draft = await message(`/cut ${sourceUrl} 0.2 0.7 --send-as-file true`);
    assert(buttons(draft.reply).length >= 9, 'ask chooser must serve all choices');
    const served = buttons(draft.reply).map(b => b.callback_data).filter(value => value.startsWith('ql:'));
    assert.equal(served.length, 10, 'chooser must offer nine quality choices and cancel');
    assert.deepEqual(new Set(served.map(value => value.split(':')[1])), new Set([...options, 'cancel']));
    for (const data of served) assert.match(data, /^ql:(?:best|2160|1440|1080|720|480|360|240|144|cancel):[0-9a-f]{32}:[1-9]\d*$/);
    assert.equal(await count(), before, 'prompting must not admit an operation');
    assert.equal(await fixtureSql(settingsSql(settingsFields)), prefsBeforePrompt, 'prompting must not write any preference field');
    assert.equal(JSON.parse(await fixtureSql(settingsSql(['quality']))).quality, 'ask', 'prompt must not mutate saved preference');
    assert.equal(await fixtureSql("SELECT stage FROM runtime_quality_sessions WHERE actor_id=42 AND bot_id=123456 AND chat_id=42"), 'choosing');
    assert.equal(sourceFixture.mediaRequests, sourceBefore, 'prompting must not fetch source metadata');
    await restartApp();
    const resume = await message('/quality@yoink_fixture_bot');
    assert(buttons(resume.reply).some(b => b.callback_data === served[0]), 'resume must expose the same durable choices');
    const beforeGroup = await count();
    const groupStart = botApi.calls.length;
    botApi.enqueue(42, -10042, `/video ${sourceUrl}`, nextMessage++, 'en');
    await message('/quality');
    assert(!botApi.calls.slice(groupStart).some(call => call.method === 'sendmessage' && Number(call.data.chat_id) === -10042), 'disabled group ingress must stay silent and never serve a quality chooser');
    assert.equal(await count(), beforeGroup, 'group ask preference must not silently admit');
    const choose144 = served.find(value => value.startsWith('ql:144:'));
    for (const invalid of ['ql:bogus:0123456789abcdef0123456789abcdef:1', 'ql:144:0123456789abcdef0123456789abcdef:0', 'ql:144:0123456789abcdef0123456789abcdef:1:extra']) {
      const denied = await callback(invalid, resume);
      assert(denied.ack.data.text || denied.ack.data.show_alert, 'malformed callback must be rejected');
    }
    const deniedSet = [
      { actor: 43 }, { flags: { isBot: true } }, { flags: { inaccessible: true } },
      { flags: { messagePatch: { message_thread_id: 9 } } },
      { flags: { messagePatch: { sender_chat: { id: -100, type: 'supergroup', title: 'Anonymous' } } } },
      { flags: { messagePatch: { chat: { id: -100, type: 'group', title: 'Foreign' } } } },
      { flags: { messagePatch: { chat: { id: 43, type: 'private', first_name: 'Foreign' } } } },
      { flags: { messagePatch: { from: { id: 999, is_bot: true, first_name: 'Foreign bot' } } } },
    ];
    for (const item of deniedSet) {
      const denied = await callback(choose144, resume, { ...(item.flags ?? {}), ...(item.actor ? { actor: item.actor } : {}) });
      assert(denied.ack.data.text || denied.ack.data.show_alert, 'foreign callback must be denied');
    }
    await fixtureSql("UPDATE users SET role='banned' WHERE id=42");
    const banned = await callback(choose144, resume);
    assert(banned.ack.data.text || banned.ack.data.show_alert);
    await fixtureSql(`UPDATE users SET role=${literal(oldUser.role)} WHERE id=42`);
    await fixtureSql("UPDATE users SET ban_until=unixepoch('now')*1000000+600000000 WHERE id=42");
    const banUntilDenied = await callback(choose144, resume);
    assert(banUntilDenied.ack.data.text || banUntilDenied.ack.data.show_alert, 'active ban must block quality admission');
    await fixtureSql('UPDATE users SET ban_until=NULL WHERE id=42');
    assert.equal(await count(), before);

    await fixtureSql("UPDATE dl_user_settings SET send_as_file=0,quality='720' WHERE user_id=42");
    botApi.setExpectedUpload({ method: 'senddocument', filename: 'media.mp4', mimeType: 'video/mp4', preserveMdat: false });
    const uploadBefore = botApi.uploadCalls;
    const click = await callback(choose144, resume);
    const operationId = await admission(click.start);
    await h.succeeded(operationId);
    assert.equal(botApi.uploadCalls, uploadBefore + 1);
    const request = await h.request(operationId);
    assert.equal(request.params.max_height, 144);
    assert.equal(request.params.send_as_file, true, 'explicit send-as-file override must survive confirmation against current false preference');
    assert.deepEqual(request.clip_range, { start_seconds: 0.2, end_seconds: 0.7 }, 'immediate clip must retain its requested range');
    const upload = botApi.calls.slice(click.start).find(c => c.method === 'senddocument');
    assert.equal(upload.reply_to_message_id, draft.id,
      'delivery must reply to original Telegram request');
    const noticeMessage = botApi.calls.slice(click.start).find(c => c.method === 'sendmessage' && /[0-9a-f]{8}-[0-9a-f-]{27}/i.test(c.data.text)).messageId;
    assert.equal(await fixtureSql(`SELECT message_id FROM runtime_progress_notices WHERE operation_id='${operationId}'`), String(noticeMessage),
      'admission response must be registered as the operation progress message');
    await h.inspectUpload('quality-picked document', { video: true, audio: true, height: 120, durationMin: 0.35, durationMax: 0.8 });
    const frozen = structuredClone(request);
    await fixtureSql("UPDATE dl_user_settings SET quality='best',send_as_file=0 WHERE user_id=42");
    const afterAdmission = await count();
    await restartApp();
    const replay = await callback(choose144, resume);
    assert.equal(await admission(replay.start), operationId);
    assert.equal(await count(), afterAdmission);
    assert.equal(botApi.uploadCalls, uploadBefore + 1);
    assert.deepEqual(await h.request(operationId), frozen, 'later preferences cannot alter admitted request');
    await fixtureSql("UPDATE runtime_quality_sessions SET stage='confirming',revision=2,operation_id=NULL WHERE actor_id=42 AND bot_id=123456 AND chat_id=42");
    await restartApp();
    const confirming = await message('/quality', { id: nextMessage++ });
    const recoveryChoice = findButton(confirming.reply, 'ql:144:');
    const recoveredSession = await session();
    const recoveryId = recoveredSession.id.replaceAll('-', '');
    for (const forbidden of [`ql:720:${recoveryId}:${recoveredSession.revision}`, `ql:cancel:${recoveryId}:${recoveredSession.revision}`]) {
      const rejected = await callback(forbidden, confirming);
      assert(rejected.ack.data.text || rejected.ack.data.show_alert, 'confirming choice cannot be changed or cancelled');
      assert.equal((await session()).stage, 'confirming');
    }
    const replacementBlocked = await message(`/video ${sourceUrl}&confirming-replacement=true`);
    assert(replacementBlocked.reply.data.text.startsWith('This request conflicts'));
    assert.equal((await session()).id, recoveredSession.id);
    const recovery = await callback(recoveryChoice, confirming);
    assert.equal(await admission(recovery.start), operationId);
    assert.equal(await count(), afterAdmission);
    assert.equal((await session()).stage, 'submitted');
    for (const data of [served.find(value => value.startsWith('ql:720:')), served.find(value => value.startsWith('ql:cancel:'))]) {
      const blocked = await callback(data, resume);
      assert(blocked.ack.data.text || blocked.ack.data.show_alert, 'submitted state cannot change quality or cancel');
    }
    botApi.setExpectedUpload(null);

    await fixtureSql("UPDATE dl_user_settings SET quality='ask' WHERE user_id=42");
    const bestDraft = await message(`/video ${sourceUrl}`);
    const bestButton = findButton(bestDraft.reply, 'ql:best:');
    const bestClick = await callback(bestButton, bestDraft);
    const bestOperation = await admission(bestClick.start);
    await h.succeeded(bestOperation);
    assert.equal((await h.request(bestOperation)).params.max_height, null, 'selected best must be stored as no maximum height');

    // A new chooser supersedes the old one. Old identifiers remain inert even after reset.
    await fixtureSql("UPDATE dl_user_settings SET quality='ask' WHERE user_id=42");
    const oldDraft = await message(`/download ${sourceUrl}`);
    const oldChoice = findButton(oldDraft.reply, 'ql:');
    const countBeforeIntentReplay = await count();
    const changedIntent = await message(`/video ${sourceUrl}&changed=true`, { id: oldDraft.id });
    assert(changedIntent.reply.data.text.startsWith('This request conflicts'), 'changed URL must be rejected, not treated as a replay');
    assert.equal(await count(), countBeforeIntentReplay, 'same message identity with changed intent cannot admit');
    assert.equal((await session()).id.replaceAll('-', ''), oldChoice.split(':')[2], 'changed intent cannot replace the original prompt');
    const replacement = await message(`/download ${sourceUrl}`);
    const newChoice = findButton(replacement.reply, 'ql:');
    assert.notEqual(oldChoice, newChoice);
    await fixtureSql("UPDATE dl_user_settings SET quality='best' WHERE user_id=42");
    const beforeStale = await count();
    const stale = await callback(oldChoice, oldDraft);
    assert(stale.ack.data.text || stale.ack.data.show_alert);
    assert.equal(await count(), beforeStale);
    await message(`/download ${sourceUrl}`, { id: oldDraft.id });
    await message(`/video --height best ${sourceUrl}`, { id: oldDraft.id });
    assert.equal(await count(), beforeStale, 'superseded source update cannot admit after ask is disabled, even with explicit height');
    assert(!botApi.calls.slice(stale.start).some(call => call.method === 'editmessagetext'
      && Number(call.data.message_id) === oldDraft.reply.messageId), 'a stale callback must not transplant replacement choices onto an obsolete message');
    const cancelData = buttons(replacement.reply).find(b => b.callback_data.startsWith('ql:cancel:')).callback_data;
    await callback(cancelData, replacement);
    assert.equal((await session()).stage, 'cancelled');
    const staleAfterReset = await callback(newChoice, replacement);
    assert(staleAfterReset.ack.data.text || staleAfterReset.ack.data.show_alert);
    assert.equal(await count(), beforeStale);

    // Explicit height and non-video commands bypass asking, while audio ignores quality.
    await fixtureSql("UPDATE dl_user_settings SET quality='ask' WHERE user_id=42");
    for (const command of [`/video ${sourceUrl}`, `/playlist ${sourceUrl}`]) {
      const operationsBeforePrompt = await count();
      const mediaBeforePrompt = sourceFixture.mediaRequests;
      const pending = await message(command);
      assert(buttons(pending.reply).some(b => b.callback_data?.startsWith('ql:')), `${command.split(' ')[0]} must offer quality choice`);
      assert.equal(await count(), operationsBeforePrompt);
      assert.equal(sourceFixture.mediaRequests, mediaBeforePrompt, 'quality choice must precede playlist/video metadata fetch');
      const cancel = findButton(pending.reply, 'ql:cancel:');
      await callback(cancel, pending);
    }
    botApi.setExpectedUpload({ method: 'sendvideo', filename: 'media.mp4', mimeType: 'video/mp4', preserveMdat: false });
    const explicit = await h.submit(`/video --height best ${sourceUrl}`, { messageId: nextMessage++ });
    await h.succeeded(explicit.operationId);
    assert.equal((await h.request(explicit.operationId)).params.max_height, null);
    const explicitHeight = await h.submit(`/video --height 144 ${sourceUrl}`, { messageId: nextMessage++ });
    await h.succeeded(explicitHeight.operationId);
    assert.equal((await h.request(explicitHeight.operationId)).params.max_height, 144);
    botApi.setExpectedUpload({ method: 'sendaudio', filename: 'audio.mp3', mimeType: 'audio/mpeg', preserveMdat: false });
    const audio = await h.submit(`/audio --audio-codec mp3 ${sourceUrl}`, { messageId: nextMessage++ });
    await h.succeeded(audio.operationId);
    assert.equal((await h.request(audio.operationId)).params.max_height, null);
    botApi.setExpectedUpload(null);
    assert.equal(await fixtureSql("SELECT count(*) FROM runtime_quality_sessions WHERE actor_id=42 AND stage='choosing'"), '0');
    const plainText = `Video ${sourceUrl}`;
    const plain = await message(plainText, { entities: [{ type: 'url', offset: plainText.indexOf(sourceUrl), length: sourceUrl.length }] });
    assert(buttons(plain.reply).some(b => b.callback_data?.startsWith('ql:')), 'visible URL with ask must serve choice buttons');
    const clipText = `${sourceUrl} 0.2 0.4`;
    const immediateClip = await message(clipText, { entities: [{ type: 'url', offset: 0, length: sourceUrl.length }] });
    assert(buttons(immediateClip.reply).some(b => b.callback_data?.startsWith('ql:')), 'immediate plain-URL clips must use quality chooser');
    const russian = await withRussianPreference(fixtureSql, async () => message(`/video ${sourceUrl}`, { language: 'en' }));
    assert(/[А-Яа-яЁё]/u.test(russian.reply.data.text), 'saved Russian language must localize chooser');
    await assertWorkspaceClean();
    return 'durable ask-quality chooser/resume, bounded callbacks, private authorization, frozen admission, restart recovery, replay, actual document output and bypass cases passed';
  } finally {
    botApi.setExpectedUpload(null);
    await fixtureSql(`UPDATE users SET ${userFields.map(f => `${f}=${literal(oldUser[f])}`).join(',')} WHERE id=42`);
    if (oldSettings.trim()) {
      const values = JSON.parse(oldSettings);
      await fixtureSql(`UPDATE dl_user_settings SET ${settingsFields.map(f => `${f}=${literal(values[f])}`).join(',')} WHERE user_id=42`);
    } else await fixtureSql('DELETE FROM dl_user_settings WHERE user_id=42');
    await fixtureSql('DELETE FROM runtime_quality_sessions WHERE actor_id=42');
    for (const row of JSON.parse(oldSessions)) {
      await fixtureSql(`INSERT INTO runtime_quality_sessions (actor_id,bot_id,chat_id,id,revision,stage,origin_message_id,last_message_id,request_json,selected,operation_id) VALUES (${row.actor_id},${row.bot_id},${row.chat_id},${literal(row.id)},${row.revision},${literal(row.stage)},${row.origin_message_id},${row.last_message_id},${literal(row.request_json)},${literal(row.selected)},${literal(row.operation_id)})`);
    }
    assert.equal(await fixtureSql(`SELECT json_object(${userFields.map(f => `'${f}',${f}`).join(',')}) FROM users WHERE id=42`), JSON.stringify(oldUser), 'user role, ban, language and revision must be restored exactly');
    assert.equal(await fixtureSql(settingsSql(settingsFields)), oldSettings, 'all settings and updated_at must be restored exactly');
    assert.equal(await fixtureSql("SELECT coalesce(json_group_array(json_object('actor_id',actor_id,'bot_id',bot_id,'chat_id',chat_id,'id',id,'revision',revision,'stage',stage,'origin_message_id',origin_message_id,'last_message_id',last_message_id,'request_json',request_json,'selected',selected,'operation_id',operation_id)),'[]') FROM runtime_quality_sessions WHERE actor_id=42"), oldSessions, 'quality sessions must be restored exactly');
  }
}
