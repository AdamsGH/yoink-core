import assert from 'node:assert/strict';
import { commandHarness, admissionNotice } from './command-harness.mjs';
import { withRussianPreference } from './language-fixture.mjs';

const heights = ['best', '2160', '1440', '1080', '720', '480', '360', '240', '144'];
const lit = value => value === null ? 'NULL' : typeof value === 'number' ? String(value) : `'${String(value).replaceAll("'", "''")}'`;

export async function runCutQualityScenarios(context) {
  const { botApi, sourcePort, sourceFixture, fixtureSql, waitFor, restartApp, assertWorkspaceClean } = context;
  const h = commandHarness(context, 19000);
  let nextMessage = 19000;
  let nextCallback = 0;
  const url = `http://127.0.0.1:${sourcePort}/media?case=cut-quality-private-marker`;
  const settingsFields = ['quality','codec','container','proxy_enabled','proxy_url','keyboard','subs_enabled','subs_auto','subs_always_ask','subs_lang','split_size','nsfw_blur','mediainfo','send_as_file','audio_codec','gallery_zip','use_pool_cookies','youtube_auth_mode','dm_topic_thread_id','args_json','created_at','updated_at'];
  const userFields = ['role','ban_until','language','updated_at'];
  const snap = fields => `SELECT json_object(${fields.map(f => `'${f}',${f}`).join(',')}) FROM dl_user_settings WHERE user_id=42`;
  const originalSettings = await fixtureSql(snap(settingsFields));
  const originalUser = JSON.parse(await fixtureSql(`SELECT json_object(${userFields.map(f => `'${f}',${f}`).join(',')}) FROM users WHERE id=42`));
  const cutColumns = (await fixtureSql("SELECT name FROM pragma_table_info('runtime_cut_sessions') ORDER BY cid")).split('\n').filter(Boolean);
  assert(cutColumns.includes('selected') && cutColumns.includes('revision'), 'cut chooser migration columns are required');
  const cutObject = `json_object(${cutColumns.map(c => `'${c}',${c}`).join(',')})`;
  const originalCuts = await fixtureSql(`SELECT coalesce(json_group_array(${cutObject}),'[]') FROM runtime_cut_sessions WHERE actor_id=42`);
  const qualityColumns = (await fixtureSql("SELECT name FROM pragma_table_info('runtime_quality_sessions') ORDER BY cid")).split('\n').filter(Boolean);
  const qualityObject = `json_object(${qualityColumns.map(c => `'${c}',${c}`).join(',')})`;
  const originalQuality = await fixtureSql(`SELECT coalesce(json_group_array(${qualityObject}),'[]') FROM runtime_quality_sessions WHERE actor_id=42`);
  let independentQuality = originalQuality;
  const qualitySnapshot = () => fixtureSql(`SELECT coalesce(json_group_array(${qualityObject}),'[]') FROM runtime_quality_sessions WHERE actor_id=42`);
  const initialOperations = await fixtureSql('SELECT count(*) FROM runtime_operations');
  const count = () => fixtureSql('SELECT count(*) FROM runtime_operations');
  const buttons = reply => (typeof reply.data.reply_markup === 'string' ? JSON.parse(reply.data.reply_markup) : reply.data.reply_markup)?.inline_keyboard?.flat() ?? [];
  async function message(text, { id = nextMessage++, language = 'en', actor = 42, chat = 42, thread } = {}) {
    const from = botApi.calls.length;
    botApi.enqueue(actor, chat, text, id, language, { thread });
    const reply = await waitFor(() => botApi.calls.slice(from).find(c => c.method === 'sendmessage' && Number(c.data.chat_id) === chat), `cut-quality reply to ${text.split(' ')[0]}`);
    assert(!reply.data.text.includes('cut-quality-private-marker'), 'chooser must not echo source URL');
    return { reply, from, id };
  }
  const button = (origin, prefix) => {
    const found = buttons(origin.reply).find(item => item.callback_data?.startsWith(prefix));
    assert(found, `expected callback button ${prefix}`);
    assert(Buffer.byteLength(found.callback_data) <= 64);
    return found.callback_data;
  };
  async function callback(data, origin, { actor = 42, chat = 42, ...flags } = {}) {
    const from = botApi.calls.length;
    const id = `cut-quality-${++nextCallback}`;
    botApi.enqueueCallback(actor, chat, origin.reply.messageId, data, { id, ...flags });
    const ack = await waitFor(() => botApi.calls.slice(from).find(c => c.method === 'answercallbackquery' && c.data.callback_query_id === id), 'cut-quality callback acknowledgement');
    return { from, ack };
  }
  async function updated(from, origin, prefix = 'cut:q') {
    const call = await waitFor(() => botApi.calls.slice(from).find(c => c.method === 'editmessagetext'
      && Number(c.data.message_id) === origin.reply.messageId && buttons(c).some(b => b.callback_data?.startsWith(prefix))), 'updated cut chooser');
    return { reply: { ...call, messageId: Number(call.data.message_id) }, id: origin.id };
  }
  async function openChooser() {
    await message(`/cut ${url} 0.2`);
    const ready = await message('0.7');
    const sent = await callback(button(ready, 'cut:go:'), ready);
    return updated(sent.from, ready);
  }
  async function admitted(from) {
    return (await admissionNotice(context, from, { session: 'cut' })).operationId;
  }
  async function waitReply(from, chat = 42) {
    return waitFor(() => botApi.calls.slice(from).find(c => c.method === 'sendmessage' && Number(c.data.chat_id) === chat), 'private response after callback');
  }
  async function session() {
    return JSON.parse(await fixtureSql(`SELECT ${cutObject} FROM runtime_cut_sessions WHERE actor_id=42 AND bot_id=123456 AND chat_id=42`));
  }
  try {
    await fixtureSql(`INSERT INTO dl_user_settings (user_id,quality,codec,container,proxy_enabled,keyboard,subs_enabled,subs_auto,subs_always_ask,subs_lang,split_size,nsfw_blur,mediainfo,send_as_file,audio_codec,gallery_zip,use_pool_cookies,youtube_auth_mode,args_json,created_at,updated_at) VALUES (42,'best','avc1','mp4',0,'2x3',0,0,0,'en',2043000000,1,0,0,'best',0,1,'cookies','{}',unixepoch('now')*1000000,unixepoch('now')*1000000) ON CONFLICT(user_id) DO UPDATE SET quality='best',codec='avc1',container='mp4',send_as_file=0,proxy_enabled=0`);
    await fixtureSql("UPDATE dl_user_settings SET quality='ask' WHERE user_id=42");
    const prefSnapshot = await fixtureSql(snap(settingsFields));
    await fixtureSql('DELETE FROM runtime_quality_sessions WHERE actor_id=42');
    const ordinary = await message(`/video ${url}&independent-quality`);
    assert(buttons(ordinary.reply).some(b => b.callback_data?.startsWith('ql:')), 'independent quality chooser is active');
    independentQuality = await qualitySnapshot();
    const sourceReads = sourceFixture.mediaRequests;
    // Before selection a draft remains editable, cancellable and replaceable.
    const editable = await openChooser();
    const obsolete = button(editable, 'cut:q144:');
    await callback(button(editable, 'cut:edit:'), editable);
    await message('/cut');
    assert.equal((await session()).stage, 'start');
    const staleEdit = await callback(obsolete, editable);
    await message('/cut');
    assert(staleEdit.ack.data.text);
    assert(!botApi.calls.slice(staleEdit.from).some(c => c.method === 'editmessagetext'
      && Number(c.data.message_id) === editable.reply.messageId), 'old revision cannot repaint a dialog');
    await message('0.2');
    const editedReady = await message('0.7');
    const editGo = await callback(button(editedReady, 'cut:go:'), editedReady);
    const cancellable = await updated(editGo.from, editedReady);
    await callback(button(cancellable, 'cut:cancel:'), cancellable);
    await message('/cut');
    assert.equal((await session()).stage, 'cancelled');
    assert((await callback(button(cancellable, 'cut:q144:'), cancellable)).ack.data.text);
    const replaceable = await openChooser();
    const replacement = await openChooser();
    const replacementRow = await session();
    assert.notEqual(button(replaceable, 'cut:q144:'), button(replacement, 'cut:q144:'));
    const staleReplacement = await callback(button(replaceable, 'cut:q144:'), replaceable);
    await message('/cut');
    assert(staleReplacement.ack.data.text);
    assert.deepEqual(await session(), replacementRow);
    assert(!botApi.calls.slice(staleReplacement.from).some(c => c.method === 'editmessagetext'
      && Number(c.data.message_id) === replaceable.reply.messageId), 'superseded message cannot acquire replacement buttons');
    await callback(button(replacement, 'cut:cancel:'), replacement);
    await message('/cut');
    for (const cancel of ['cancel', 'отмена', '/cancel']) {
      const pending = await openChooser();
      await message(cancel);
      assert.equal((await session()).stage, 'cancelled', `${cancel} cancels an unselected quality draft`);
      assert((await callback(button(pending, 'cut:q144:'), pending)).ack.data.text);
    }
    assert.equal(await count(), initialOperations);
    const start = await message(`/cut ${url}`);
    await message('0.2');
    const ready = await message('0.7');
    const go = buttons(ready.reply).find(b => b.callback_data?.startsWith('cut:go:'))?.callback_data;
    assert(go, 'ready interactive cut must expose Go');
    const countBeforeChoice = await count();
    const choose = await callback(go, ready);
    const choiceMenu = await updated(choose.from, ready);
    const served = buttons(choiceMenu.reply).filter(b => /^cut:q(?:best|\d+):[0-9a-f]{32}:[1-9]\d*$/.test(b.callback_data ?? ''));
    assert.equal(served.length, 9, 'cut chooser serves all fixed heights');
    for (const height of heights) assert(served.some(b => b.callback_data.startsWith(`cut:q${height === 'best' ? 'best' : height}:`)), `missing ${height} choice`);
    const cutId = served[0].callback_data.split(':')[2];
    const revision = served[0].callback_data.split(':')[3];
    assert(served.every(b => b.callback_data.split(':')[2] === cutId && b.callback_data.split(':')[3] === revision));
    assert.equal(await count(), countBeforeChoice, 'opening chooser cannot admit');
    assert.equal(sourceFixture.mediaRequests, sourceReads, 'chooser cannot read source');
    assert.equal(await fixtureSql(snap(settingsFields)), prefSnapshot, 'selection flow cannot mutate preferences');
    assert.equal(JSON.parse(await fixtureSql(snap(['quality']))).quality, 'ask');
    assert.equal((await session()).stage, 'quality');
    await restartApp();
    const resumed = await message('/cut');
    assert(served.every(s => buttons(resumed.reply).some(b => b.callback_data === s.callback_data)), 'cut chooser persists across restart');
    const malformed = await callback(`cut:q144:${cutId}:0`, resumed);
    assert(malformed.ack.data.text || malformed.ack.data.show_alert);
    const selected144 = served.find(b => b.callback_data.startsWith('cut:q144:')).callback_data;
    for (const invalid of [
      { actor: 43 }, { isBot: true }, { inaccessible: true },
      { messagePatch: { message_thread_id: 9 } },
      { messagePatch: { sender_chat: { id: -100, type: 'supergroup', title: 'Anonymous' } } },
      { messagePatch: { chat: { id: -100, type: 'group', title: 'Foreign' } } },
      { messagePatch: { chat: { id: 43, type: 'private', first_name: 'Foreign' } } },
      { messagePatch: { from: { id: 999, is_bot: true, first_name: 'Foreign bot' } } },
    ]) {
      const denied = await callback(selected144, resumed, invalid);
      assert(denied.ack.data.text || denied.ack.data.show_alert, 'foreign callback is denied');
    }
    await fixtureSql("UPDATE users SET role='banned' WHERE id=42");
    const roleDenied = await callback(selected144, resumed);
    assert(roleDenied.ack.data.text || roleDenied.ack.data.show_alert);
    await fixtureSql(`UPDATE users SET role=${lit(originalUser.role)},ban_until=unixepoch('now')*1000000+600000000 WHERE id=42`);
    const banDenied = await callback(selected144, resumed);
    assert(banDenied.ack.data.text || banDenied.ack.data.show_alert);
    await fixtureSql(`UPDATE users SET ban_until=${lit(originalUser.ban_until)} WHERE id=42`);
    assert.equal(await count(), initialOperations);

    // Fail after persisting the choice but before any admission, then recover after restart.
    await fixtureSql("UPDATE dl_user_settings SET quality='720',send_as_file=1,codec='invalid' WHERE user_id=42");
    const failedChoice = await callback(selected144, resumed);
    assert(failedChoice.ack.data.text, 'invalid saved codec reports failed admission');
    assert.equal((await session()).stage, 'confirming');
    assert.equal((await session()).selected, '144');
    assert.equal(await count(), initialOperations, 'pre-admission failure creates no operation');
    assert.equal(sourceFixture.mediaRequests, sourceReads, 'no source contact before successful admission');
    await fixtureSql("UPDATE dl_user_settings SET codec='avc1' WHERE user_id=42");
    await restartApp();
    const beforeAdmissionRecovery = await message('/cut');
    assert.equal(buttons(beforeAdmissionRecovery.reply).length, 1);
    const resume144 = button(beforeAdmissionRecovery, 'cut:q144:');
    botApi.setExpectedUpload({ method: 'senddocument', filename: 'media.mp4', mimeType: 'video/mp4', preserveMdat: false });
    const uploads = botApi.uploadCalls;
    const choice = await callback(resume144, beforeAdmissionRecovery);
    const op = await admitted(choice.from);
    await h.succeeded(op);
    assert.equal(botApi.uploadCalls, uploads + 1);
    const request = await h.request(op);
    assert.deepEqual(request.clip_range, { start_seconds: 0.2, end_seconds: 0.7 });
    assert.equal(request.params.max_height, 144);
    const upload = botApi.calls.slice(choice.from).find(c => c.method === 'senddocument');
    assert.equal(upload.reply_to_message_id, start.id, 'clip delivery retains original routing message');
    await h.inspectUpload('quality-selected clipped document', { video: true, audio: true, durationMin: 0.35, durationMax: 0.8, height: 120 });
    const frozen = structuredClone(request);
    await fixtureSql("UPDATE dl_user_settings SET quality='best',send_as_file=0 WHERE user_id=42");
    const afterAdmission = await count();
    await restartApp();
    const replay = await callback(selected144, resumed);
    assert.equal(await admitted(replay.from), op);
    assert.equal(await count(), afterAdmission);
    assert.equal(botApi.uploadCalls, uploads + 1);
    assert.deepEqual(await h.request(op), frozen, 'replay preserves frozen selected operation');

    await fixtureSql("UPDATE runtime_cut_sessions SET stage='confirming',operation_id=NULL WHERE actor_id=42 AND bot_id=123456 AND chat_id=42");
    await restartApp();
    const confirmingMenu = await message('/cut');
    const current = await session();
    assert.equal(current.stage, 'confirming');
    assert.equal(current.selected, '144');
    assert.equal(current.revision, Number(revision) + 1);
    const different = served.find(b => b.callback_data.startsWith('cut:q720:')).callback_data;
    for (const forbidden of [different, `cut:edit:${cutId}:${revision}`, `cut:cancel:${cutId}:${revision}`]) {
      const denied = await callback(forbidden, resumed);
      assert(denied.ack.data.text || denied.ack.data.show_alert);
      assert.equal((await session()).stage, 'confirming');
    }
    const replacementStart = botApi.calls.length;
    botApi.enqueue(42, 42, `/cut ${url}`, nextMessage++);
    const replacementResponse = await waitReply(replacementStart);
    assert(!buttons(replacementResponse).length, 'confirming intent cannot be replaced');
    assert.equal((await session()).id.replaceAll('-', ''), cutId);
    // A stale revision must not repaint an obsolete callback message with replacement controls.
    const staleRevision = await callback(`cut:q144:${cutId}:${Number(revision) - 1}`, resumed);
    assert(staleRevision.ack.data.text || staleRevision.ack.data.show_alert);
    const privateCheckStart = botApi.calls.length;
    const privateCheck = await message('/cut');
    assert(privateCheck.from >= privateCheckStart);
    assert.equal((await session()).stage, 'confirming');
    assert(!botApi.calls.slice(staleRevision.from).some(c => c.method === 'editmessagetext' && Number(c.data.message_id) === resumed.reply.messageId), 'stale callback cannot transplant a menu');
    assert.deepEqual(buttons(confirmingMenu.reply).map(b => b.callback_data), [`cut:q144:${cutId}:${current.revision}`], 'recovery offers only the selected height');
    const recoveryChoice = button(confirmingMenu, 'cut:q144:');
    const recovered = await callback(recoveryChoice, confirmingMenu);
    assert.equal(await admitted(recovered.from), op);
    assert.equal(await count(), afterAdmission);
    assert.equal(botApi.uploadCalls, uploads + 1);
    assert.equal((await session()).stage, 'submitted');

    await fixtureSql("UPDATE dl_user_settings SET quality='ask' WHERE user_id=42");
    await message(`/cut ${url}?best-choice`);
    await message('0.2');
    const bestReady = await message('0.7');
    const bestGo = await callback(button(bestReady, 'cut:go:'), bestReady);
    const bestOrigin = await updated(bestGo.from, bestReady);
    const bestData = buttons(bestOrigin.reply).find(b => b.callback_data.startsWith('cut:qbest:')).callback_data;
    botApi.setExpectedUpload({ method: 'sendvideo', filename: 'media.mp4', mimeType: 'video/mp4', preserveMdat: false });
    const bestClick = await callback(bestData, bestOrigin);
    const bestOp = await admitted(bestClick.from);
    await h.succeeded(bestOp);
    assert.equal((await h.request(bestOp)).params.max_height, null);
    assert.equal(await fixtureSql("SELECT quality FROM dl_user_settings WHERE user_id=42"), 'ask', 'height selection preserves saved ask');
    botApi.setExpectedUpload(null);
    const russian = await withRussianPreference(fixtureSql, openChooser);
    assert(/[А-Яа-яЁё]/u.test(russian.reply.data.text), 'cut quality chooser is localized');
    await callback(button(russian, 'cut:cancel:'), russian);
    await message('/cut');
    assert.equal(await qualitySnapshot(), independentQuality, 'active regular quality dialog remains unchanged');
    await assertWorkspaceClean();
    return 'native cut-quality chooser, persistence, authorization, frozen preferences, real clipped output and same-choice crash recovery verified';
  } finally {
    botApi.setExpectedUpload(null);
    await fixtureSql(`UPDATE users SET ${userFields.map(f => `${f}=${lit(originalUser[f])}`).join(',')} WHERE id=42`);
    if (originalSettings.trim()) {
      const row = JSON.parse(originalSettings);
      await fixtureSql(`UPDATE dl_user_settings SET ${settingsFields.map(f => `${f}=${lit(row[f])}`).join(',')} WHERE user_id=42`);
    } else await fixtureSql('DELETE FROM dl_user_settings WHERE user_id=42');
    await fixtureSql('DELETE FROM runtime_quality_sessions WHERE actor_id=42');
    await fixtureSql('DELETE FROM runtime_cut_sessions WHERE actor_id=42');
    for (const row of JSON.parse(originalCuts)) {
      await fixtureSql(`INSERT INTO runtime_cut_sessions (${cutColumns.join(',')}) VALUES (${cutColumns.map(c => lit(row[c])).join(',')})`);
    }
    for (const row of JSON.parse(originalQuality)) {
      await fixtureSql(`INSERT INTO runtime_quality_sessions (${qualityColumns.join(',')}) VALUES (${qualityColumns.map(c => lit(row[c])).join(',')})`);
    }
    assert.equal(await fixtureSql(`SELECT coalesce(json_group_array(${cutObject}),'[]') FROM runtime_cut_sessions WHERE actor_id=42`), originalCuts, 'cut rows restored');
    assert.equal(await fixtureSql(`SELECT json_object(${userFields.map(f => `'${f}',${f}`).join(',')}) FROM users WHERE id=42`), JSON.stringify(originalUser), 'user settings and revisions restored');
    assert.equal(await fixtureSql(snap(settingsFields)), originalSettings, 'all user preferences and revisions restored');
    assert.equal(await fixtureSql(`SELECT coalesce(json_group_array(${qualityObject}),'[]') FROM runtime_quality_sessions WHERE actor_id=42`), originalQuality, 'regular quality sessions restored unchanged');
  }
}
