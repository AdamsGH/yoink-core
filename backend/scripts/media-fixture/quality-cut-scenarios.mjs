import assert from 'node:assert/strict';
import { commandHarness, admissionNotice } from './command-harness.mjs';
import { withRussianPreference } from './language-fixture.mjs';

const lit = value => value === null ? 'NULL' : typeof value === 'number' ? String(value) : `'${String(value).replaceAll("'", "''")}'`;

export async function runQualityCutScenarios(context) {
  const { botApi, sourcePort, sourceFixture, fixtureSql, waitFor, restartApp, assertWorkspaceClean } = context;
  const h = commandHarness(context, 20000);
  const url = `http://127.0.0.1:${sourcePort}/media?case=quality-cut-private-marker`;
  let nextMessage = 20000;
  let nextCallback = 0;
  const userFields = ['role', 'ban_until', 'language', 'updated_at'];
  const settingsFields = ['quality','codec','container','proxy_enabled','proxy_url','keyboard','subs_enabled','subs_auto','subs_always_ask','subs_lang','split_size','nsfw_blur','mediainfo','send_as_file','audio_codec','gallery_zip','use_pool_cookies','youtube_auth_mode','dm_topic_thread_id','args_json','created_at','updated_at'];
  const columns = async table => (await fixtureSql(`SELECT name FROM pragma_table_info('${table}') ORDER BY cid`)).split('\n').filter(Boolean);
  const snapshot = async (table, fields) => fixtureSql(`SELECT coalesce(json_group_array(json_object(${fields.map(f => `'${f}',${f}`).join(',')})),'[]') FROM ${table} WHERE actor_id=42`);
  const restore = async (table, fields, rows) => {
    await fixtureSql(`DELETE FROM ${table} WHERE actor_id=42`);
    for (const row of rows) await fixtureSql(`INSERT INTO ${table} (${fields.join(',')}) VALUES (${fields.map(f => lit(row[f])).join(',')})`);
  };
  const qualityFields = await columns('runtime_quality_sessions');
  const cutFields = await columns('runtime_cut_sessions');
  const originalQuality = await snapshot('runtime_quality_sessions', qualityFields);
  const originalCuts = await snapshot('runtime_cut_sessions', cutFields);
  const settingsSql = fields => `SELECT json_object(${fields.map(f => `'${f}',${f}`).join(',')}) FROM dl_user_settings WHERE user_id=42`;
  const originalSettings = await fixtureSql(settingsSql(settingsFields));
  const originalUser = JSON.parse(await fixtureSql(`SELECT json_object(${userFields.map(f => `'${f}',${f}`).join(',')}) FROM users WHERE id=42`));
  const originalOperations = await fixtureSql('SELECT count(*) FROM runtime_operations');
  const buttons = reply => {
    const markup = reply.data.reply_markup;
    return (typeof markup === 'string' ? JSON.parse(markup) : markup)?.inline_keyboard?.flat() ?? [];
  };
  async function message(text, { id = nextMessage++, language = 'en', actor = 42, chat = 42, ...flags } = {}) {
    const from = botApi.calls.length;
    botApi.enqueue(actor, chat, text, id, language, flags);
    const reply = await waitFor(() => botApi.calls.slice(from).find(c => c.method === 'sendmessage' && Number(c.data.chat_id) === chat), `quality-cut reply to ${text.split(' ')[0]}`);
    assert(!reply.data.text.includes('quality-cut-private-marker'), 'fixture marker cannot be echoed');
    return { reply, from, id };
  }
  async function callback(data, origin, { actor = 42, chat = 42, ...flags } = {}) {
    const from = botApi.calls.length;
    const id = `quality-cut-${++nextCallback}`;
    botApi.enqueueCallback(actor, chat, origin.reply.messageId, data, { id, ...flags });
    const ack = await waitFor(() => botApi.calls.slice(from).find(c => c.method === 'answercallbackquery' && c.data.callback_query_id === id), 'quality-cut callback acknowledgement');
    return { from, ack };
  }
  const button = (origin, prefix) => {
    const found = buttons(origin.reply).find(b => b.callback_data?.startsWith(prefix));
    assert(found, `expected served callback ${prefix}`);
    assert(Buffer.byteLength(found.callback_data) <= 64);
    return found.callback_data;
  };
  const count = () => fixtureSql('SELECT count(*) FROM runtime_operations');
  const cutRow = async () => JSON.parse(await fixtureSql(`SELECT json_object(${cutFields.map(f => `'${f}',${f}`).join(',')}) FROM runtime_cut_sessions WHERE actor_id=42 AND bot_id=123456 AND chat_id=42`));
  const qualityRow = async () => JSON.parse(await fixtureSql(`SELECT json_object(${qualityFields.map(f => `'${f}',${f}`).join(',')}) FROM runtime_quality_sessions WHERE actor_id=42 AND bot_id=123456 AND chat_id=42`));
  try {
    await fixtureSql("UPDATE users SET language='en' WHERE id=42");
    await fixtureSql(`INSERT INTO dl_user_settings (user_id,quality,codec,container,proxy_enabled,keyboard,subs_enabled,subs_auto,subs_always_ask,subs_lang,split_size,nsfw_blur,mediainfo,send_as_file,audio_codec,gallery_zip,use_pool_cookies,youtube_auth_mode,args_json,created_at,updated_at) VALUES (42,'best','avc1','mp4',0,'2x3',0,0,0,'en',2043000000,1,0,0,'best',0,1,'cookies','{}',unixepoch('now')*1000000,unixepoch('now')*1000000) ON CONFLICT(user_id) DO UPDATE SET quality='ask',codec='avc1',container='mp4',send_as_file=0,proxy_enabled=0`);
    await fixtureSql("UPDATE dl_user_settings SET quality='ask',codec='avc1',container='mp4',send_as_file=0 WHERE user_id=42");
    const sourceReads = sourceFixture.mediaRequests;
    const request = await message(`/cut ${url} 0.2 0.7 --send-as-file true --video-codec h264 --container mp4`, { language: 'en' });
    assert(!/[А-Яа-яЁё]/u.test(request.reply.data.text), 'quality prompt is English under English preference');
    const cutChoice = button(request, 'ql:cut:');
    assert.match(cutChoice, /^ql:cut:[0-9a-f]{32}:[1-9]\d*$/);
    const quality = await qualityRow();
    assert.equal(quality.stage, 'choosing');
    const choiceSnapshot = await fixtureSql(settingsSql(settingsFields));
    const startCount = await count();
    const invalidContexts = [
      { actor: 43 }, { isBot: true }, { inaccessible: true },
      { messagePatch: { message_thread_id: 9 } },
      { messagePatch: { sender_chat: { id: -100, type: 'supergroup', title: 'Anonymous' } } },
      { messagePatch: { chat: { id: -100, type: 'group', title: 'Foreign' } } },
      { messagePatch: { chat: { id: 43, type: 'private', first_name: 'Foreign' } } },
      { messagePatch: { from: { id: 999, is_bot: true, first_name: 'Foreign bot' } } },
    ];
    for (const invalid of invalidContexts) {
      const denied = await callback(cutChoice, request, invalid);
      assert(denied.ack.data.text || denied.ack.data.show_alert, 'Cut handoff checks private owner and callback origin');
    }
    await fixtureSql("UPDATE users SET role='banned' WHERE id=42");
    const roleDenied = await callback(cutChoice, request);
    assert(roleDenied.ack.data.text || roleDenied.ack.data.show_alert);
    await fixtureSql(`UPDATE users SET role=${lit(originalUser.role)},ban_until=unixepoch('now')*1000000+600000000 WHERE id=42`);
    const banDenied = await callback(cutChoice, request);
    assert(banDenied.ack.data.text || banDenied.ack.data.show_alert);
    await fixtureSql(`UPDATE users SET ban_until=${lit(originalUser.ban_until)} WHERE id=42`);
    assert.equal((await qualityRow()).stage, 'choosing', 'denied callbacks leave the quality draft intact');
    const selected = await callback(cutChoice, request);
    assert(!selected.ack.data.text || !/error/i.test(selected.ack.data.text));
    assert.equal(await count(), startCount, 'Cut handoff cannot admit an operation');
    assert.equal(sourceFixture.mediaRequests, sourceReads, 'Cut handoff cannot fetch source metadata');
    assert.equal(await fixtureSql(settingsSql(settingsFields)), choiceSnapshot, 'Cut handoff cannot mutate preferences');
    const handedQuality = await qualityRow();
    const draft = await cutRow();
    assert.equal(handedQuality.stage, 'cutting');
    assert.equal(handedQuality.linked_cut_id, draft.id);
    assert.equal(JSON.stringify(JSON.parse(draft.original_request_json)), JSON.stringify(JSON.parse(handedQuality.request_json)), 'linked cut preserves original request');
    const original = JSON.parse(draft.original_request_json);
    assert.equal(original.params.send_as_file, false, 'DownloadParams retain codec defaults');
    assert.equal(original.params.video_codec, 'h264');
    assert.equal(original.params.container, 'mp4');
    assert.deepEqual(original.preference_overrides, {
      video_codec: 'h264', audio_codec: null, container: 'mp4', send_as_file: true,
      gallery_zip: null,
    }, 'explicit format options persist separately from DownloadParams');
    assert.deepEqual(original.clip_range, { start_seconds: 0.2, end_seconds: 0.7 }, 'handoff carries an initial clip-range seed');

    await restartApp();
    const resumed = await message('/quality');
    assert.equal((await count()), startCount);
    const resumedActions = buttons(resumed.reply).map(item => item.callback_data ?? '');
    assert(resumedActions.some(value => value.startsWith('cut:go:'))
      && resumedActions.some(value => value.startsWith('cut:edit:'))
      && resumedActions.some(value => value.startsWith('cut:cancel:')),
    '/quality Cutting renders the linked Ready editor, not a quality chooser');
    assert(resumedActions.every(value => value.startsWith('cut:'))
      && !resumedActions.some(value => /^ql:(?:audio|\d+):/.test(value)),
    'linked Cut resume has no original quality Audio or height choices');
    assert.equal((await cutRow()).id, draft.id, '/quality resumes the exact linked Cut session');
    assert.equal(await count(), startCount, '/quality linked-editor resume cannot admit an operation');
    const oldChoice = request;
    const staleAudio = await callback(button(oldChoice, 'ql:audio:'), oldChoice);
    assert(staleAudio.ack.data.text || staleAudio.ack.data.show_alert);
    const staleHeight = await callback(button(oldChoice, 'ql:720:'), oldChoice);
    assert(staleHeight.ack.data.text || staleHeight.ack.data.show_alert);
    assert.equal((await qualityRow()).stage, 'cutting');

    // Resume the linked editor through the public command, then complete a real clip.
    const russian = await withRussianPreference(fixtureSql, () => message('/cut', { language: 'en' }));
    assert(/[А-Яа-яЁё]/u.test(russian.reply.data.text), 'linked Cut editor honors Russian preference');
    const editor = await message('/cut');
    assert(/clip|start|cut/i.test(editor.reply.data.text));
    const editorActions = buttons(editor.reply).map(item => item.callback_data ?? '');
    assert(editorActions.some(value => value.startsWith('cut:edit:')) && editorActions.some(value => value.startsWith('cut:cancel:')),
      'linked Cut editor exposes its served Edit and Cancel actions');
    assert(editorActions.some(value => value.startsWith('cut:go:')), 'seeded linked Cut editor exposes Go');
    const currentCut = await cutRow();
    assert.equal(currentCut.id, draft.id, 'restart reopens the linked draft, not a replacement');
    assert.equal(await count(), startCount);
    const edit = buttons(editor.reply).find(b => b.callback_data?.startsWith('cut:edit:'))?.callback_data;
    if (edit) {
      const editing = await callback(edit, editor);
      await waitFor(() => botApi.calls.slice(editing.from).find(c => ['sendmessage','editmessagetext'].includes(c.method)
        && /start/i.test(c.data.text)), 'seeded Cut edit prompt');
    }
    await message('0.2');
    await restartApp();
    const endTime = await message('0.7');
    assert(/clip|summary|confirm/i.test(endTime.reply.data.text));
    const go = buttons(endTime.reply).find(b => b.callback_data?.startsWith('cut:go:'))?.callback_data;
    assert(go, 'completed handoff draft serves the existing Go action');
    assert.equal(await count(), startCount, 'entering times and opening summary cannot admit');
    const goResult = await callback(go, endTime);
    const qualityMenuCall = await waitFor(() => botApi.calls.slice(goResult.from).find(c => c.method === 'editmessagetext'
      && Number(c.data.message_id) === endTime.reply.messageId && buttons(c).some(b => b.callback_data?.startsWith('cut:q144:'))), 'Cut quality chooser after handoff');
    const chooser = { reply: { ...qualityMenuCall, messageId: Number(qualityMenuCall.data.message_id) }, id: endTime.id };
    const q144 = button(chooser, 'cut:q144:');
    botApi.setExpectedUpload({ method: 'senddocument', filename: 'media.mp4', mimeType: 'video/mp4', preserveMdat: false });
    const uploads = botApi.uploadCalls;
    const picked = await callback(q144, chooser);
    const { operationId } = await admissionNotice(context, picked.from, { session: 'cut' });
    await h.succeeded(operationId);
    assert.equal(botApi.uploadCalls, uploads + 1, 'handoff performs one real upload');
    const admittedRequest = await h.request(operationId);
    assert.deepEqual(admittedRequest.clip_range, { start_seconds: 0.2, end_seconds: 0.7 });
    assert.equal(admittedRequest.params.max_height, 144);
    assert.equal(admittedRequest.params.send_as_file, true, 'original explicit file override survives handoff');
    assert.equal(admittedRequest.params.video_codec, 'h264');
    assert.equal(admittedRequest.params.container, 'mp4');
    assert.equal(admittedRequest.target.reply_to_message_id, request.id, 'operation retains original reply target');
    const upload = botApi.calls.slice(picked.from).find(c => c.method === 'senddocument');
    assert.equal(upload.reply_to_message_id, request.id, 'delivery replies to original Bot API message');
    await h.inspectUpload('quality-to-Cut video document', { video: true, audio: true, durationMin: 0.35, durationMax: 0.8, height: 120 });
    const frozen = structuredClone(admittedRequest);
    const admittedCount = await count();
    await fixtureSql("UPDATE dl_user_settings SET quality='best',send_as_file=0,codec='invalid',container='invalid' WHERE user_id=42");
    const changedPreferences = await fixtureSql(settingsSql(settingsFields));
    await fixtureSql(`UPDATE runtime_cut_sessions SET stage='confirming',operation_id=NULL WHERE id=${lit(draft.id)}`);
    await restartApp();
    const recovering = await message('/cut');
    const recoveryChoice = button(recovering, 'cut:q144:');
    const replay = await callback(recoveryChoice, recovering);
    const { operationId: replayedOperation } = await admissionNotice(context, replay.from, { session: 'cut' });
    assert.equal(replayedOperation, operationId, 'admission crash recovery reuses the linked operation');
    assert.equal(await count(), admittedCount, 'handoff recovery cannot create a second operation');
    assert.equal(botApi.uploadCalls, uploads + 1, 'handoff replay cannot upload twice');
    assert.deepEqual(await h.request(operationId), frozen, 'changed settings cannot alter admitted request');
    assert.equal(await fixtureSql(settingsSql(settingsFields)), changedPreferences, 'replay cannot rewrite current settings');
    assert.equal((await cutRow()).stage, 'submitted');
    const qualityBeforeCommandReplay = await qualityRow();
    await message(`/cut ${url} 0.2 0.7 --send-as-file true --video-codec h264 --container mp4`, { id: request.id });
    assert.deepEqual(await qualityRow(), qualityBeforeCommandReplay, 'replaying the original command after handoff cannot recreate or replace its quality row');
    const forgedQualityCut = button(request, 'ql:cut:');
    const submittedCutSnapshot = await cutRow();
    for (const stage of ['confirming', 'submitted', 'cancelled']) {
      await fixtureSql(`UPDATE runtime_quality_sessions SET stage='${stage}',revision=${stage === 'submitted' ? 3 : 2},selected=${stage === 'cancelled' ? 'NULL' : "'144'"},operation_id=${stage === 'submitted' ? lit(operationId) : 'NULL'},linked_cut_id=NULL WHERE actor_id=42 AND bot_id=123456 AND chat_id=42`);
      const forgedQualitySnapshot = await qualityRow();
      const forged = await callback(forgedQualityCut, request);
      assert(forged.ack.data.text || forged.ack.data.show_alert, `forged qlCut is rejected while quality is ${stage}`);
      assert.deepEqual(await cutRow(), submittedCutSnapshot, `forged qlCut cannot transfer or create a Cut while quality is ${stage}`);
      assert.deepEqual(await qualityRow(), forgedQualitySnapshot, `forged qlCut cannot change the ${stage} quality row`);
    }
    botApi.setExpectedUpload(null);
    await restartApp();
    await message('/cut');
    assert.equal((await cutRow()).id, draft.id);

    // Restore valid ask settings after proving replay ignores the deliberately invalid preferences.
    await fixtureSql("UPDATE dl_user_settings SET quality='ask',codec='avc1',container='mp4' WHERE user_id=42");

    // A newer unrelated active Cut blocks transfer without changing either durable row.
    const unrelated = await message(`/cut ${url}`);
    const unrelatedStart = await cutRow();
    assert.equal(unrelatedStart.stage, 'start');
    await message('0.1');
    await message('0.3');
    const blockedQuality = await message(`/video ${url}&blocked-quality`);
    assert.match(blockedQuality.reply.data.text, /Finish or cancel the active clip with \/cut before starting another clip\./,
      'chooser hides Cut and explains that an older active clip must be finished or cancelled');
    const blockedActions = buttons(blockedQuality.reply).map(item => item.callback_data ?? '');
    assert(!blockedActions.some(value => value.startsWith('ql:cut:')));
    for (const prefix of ['ql:audio:', 'ql:cancel:']) button(blockedQuality, prefix);
    const blockedQualityRow = await qualityRow();
    const blockedChoice = `ql:cut:${blockedQualityRow.id.replaceAll('-', '')}:${blockedQualityRow.revision}`;
    const beforeBlockedQuality = await qualityRow();
    const beforeBlockedCut = await cutRow();
    const beforeBlockedOperations = await count();
    const beforeBlockedSource = sourceFixture.mediaRequests;
    const blocked = await callback(blockedChoice, blockedQuality);
    assert.match(blocked.ack.data.text, /Finish or cancel the active clip with \/cut before starting another clip\./,
      'an unrelated active Cut returns the localized active-draft guidance');
    assert.deepEqual(await qualityRow(), beforeBlockedQuality, 'refused handoff preserves quality row');
    assert.deepEqual(await cutRow(), beforeBlockedCut, 'refused handoff preserves unrelated Cut row');
    assert.equal(await count(), beforeBlockedOperations, 'refused handoff cannot admit an operation');
    assert.equal(sourceFixture.mediaRequests, beforeBlockedSource, 'refused handoff cannot fetch source');
    const editingOlderCut = await message('/cut');
    await callback(button(editingOlderCut, 'cut:edit:'), editingOlderCut);
    await message('0.15');
    const activeWatermarkedCut = await cutRow();
    assert.equal(activeWatermarkedCut.stage, 'end');
    const activeWatermarkRetry = await callback(blockedChoice, blockedQuality);
    assert.match(activeWatermarkRetry.ack.data.text, /This request can no longer open the clip editor/i,
      'watermark refusal takes precedence even while the older Cut is active');
    assert.deepEqual(await cutRow(), activeWatermarkedCut, 'watermark refusal preserves the active Cut');
    assert.deepEqual(await qualityRow(), beforeBlockedQuality, 'watermark refusal preserves the chooser');
    await message('/cancel');
    assert.equal((await cutRow()).stage, 'cancelled');
    const afterBareCancelQuality = await qualityRow();
    const afterBareCancelCut = await cutRow();
    const staleAfterBareCancel = await callback(blockedChoice, blockedQuality);
    assert.match(staleAfterBareCancel.ack.data.text, /This request can no longer open the clip editor/i,
      'bare /cancel advances the older Cut watermark and gives actionable stale-link guidance');
    assert.deepEqual(await qualityRow(), afterBareCancelQuality, 'stale retry preserves quality chooser row');
    assert.deepEqual(await cutRow(), afterBareCancelCut, 'stale retry preserves cancelled Cut row');
    const staleRefresh = await waitFor(() => botApi.calls.slice(staleAfterBareCancel.from).find(c => c.method === 'editmessagetext'
      && c.data.message_id === blockedQuality.reply.messageId), 'watermarked chooser refresh');
    assert.match(staleRefresh.data.text, /This request can no longer open the clip editor/i);
    assert(!buttons(staleRefresh).some(item => item.callback_data?.startsWith('ql:cut:')), 'callback refresh cannot serve a stale Cut action');
    const watermarkedChooser = await message('/quality');
    assert.match(watermarkedChooser.reply.data.text, /This request can no longer open the clip editor/i);
    assert(!buttons(watermarkedChooser.reply).some(item => item.callback_data?.startsWith('ql:cut:')), '/quality cannot serve a stale Cut action');
    for (const prefix of ['ql:144:', 'ql:audio:', 'ql:cancel:']) button(watermarkedChooser, prefix);
    const retryQuality = await message(`/video ${url}&retry-after-cancel`);
    const retryChoice = button(retryQuality, 'ql:cut:');
    await callback(retryChoice, retryQuality);
    const retryLinked = await qualityRow();
    assert.equal(retryLinked.stage, 'cutting');
    await message('0.2');
    const linkedCancelMessage = await message('0.7');
    const linkedDraftBeforeCancel = await cutRow();
    const servedCutCancel = button(linkedCancelMessage, 'cut:cancel:');
    await callback(servedCutCancel, linkedCancelMessage);
    const cancelledLinkedDraft = await cutRow();
    assert.equal(cancelledLinkedDraft.stage, 'cancelled', 'served Cut Cancel terminates linked draft');
    assert.equal(cancelledLinkedDraft.id, linkedDraftBeforeCancel.id);
    const cancelledQuality = await qualityRow();
    assert.equal(cancelledQuality.linked_cut_id, cancelledLinkedDraft.id);
    const replayCancelled = await callback(retryChoice, retryQuality);
    const cancelledResume = await waitFor(() => botApi.calls.slice(replayCancelled.from).find(c => c.method === 'sendmessage'
      && /clip cancelled/i.test(c.data.text)), 'linked cancelled draft status after original qlCut replay');
    assert.equal(buttons(cancelledResume).length, 0, 'cancelled linked replay exposes no actions that could resurrect the draft');
    assert.deepEqual(await cutRow(), cancelledLinkedDraft, 'original qlCut replay preserves the exact terminal cancelled row');
    assert.deepEqual(await qualityRow(), cancelledQuality, 'cancelled linked replay preserves its quality link');
    assert.equal(await count(), beforeBlockedOperations, 'cancelled linked replay cannot admit another operation');
    assert.equal(cancelledLinkedDraft.original_request_json, null, 'terminal resume does not need an original request');
    const terminalCommandResume = await message('/quality');
    assert.match(terminalCommandResume.reply.data.text, /clip cancelled/i, 'valid terminal link resumes through the command');

    // Keep the forward ID intact and independently break the reverse link.
    for (const reverseLink of [null, '00000000-0000-4000-8000-000000000001']) {
      assert.notEqual(reverseLink, cancelledQuality.id, 'corrupt reverse link does not name this quality draft');
      await fixtureSql(`UPDATE runtime_cut_sessions SET linked_quality_id=${lit(reverseLink)} WHERE actor_id=42 AND bot_id=123456 AND chat_id=42`);
      const corruptCut = await cutRow();
      assert.equal(corruptCut.id, cancelledQuality.linked_cut_id, 'forward Cut ID still matches');
      const commandResume = await message('/quality');
      assert.match(commandResume.reply.data.text, /This request can no longer open the clip editor\. Send the URL again, or start \/cut URL\./,
        'command rejects missing or mismatched reverse link despite matching forward ID');
      assert.equal(buttons(commandResume.reply).length, 0, 'broken reverse link exposes no command resume actions');
      const rejectedResume = await callback(retryChoice, retryQuality);
      assert(rejectedResume.ack.data.text || rejectedResume.ack.data.show_alert, 'callback rejects the broken reverse link');
      const brokenLinkRefresh = await waitFor(() => botApi.calls.slice(rejectedResume.from).find(c => c.method === 'editmessagetext'
        && c.data.message_id === retryQuality.reply.messageId), 'broken reverse-link chooser refresh');
      assert.match(brokenLinkRefresh.data.text, /This request can no longer open the clip editor/i);
      assert(!buttons(brokenLinkRefresh).some(item => item.callback_data?.startsWith('ql:cut:')), 'refresh hides Resume Cut for a broken reverse link');
      assert.deepEqual(await cutRow(), corruptCut, 'rejected command and callback do not mutate the Cut row');
      assert.deepEqual(await qualityRow(), cancelledQuality, 'rejected command and callback preserve the quality row');
      assert.equal(await count(), beforeBlockedOperations, 'broken reverse-link resume cannot admit a job');
    }
    await fixtureSql(`UPDATE runtime_cut_sessions SET linked_quality_id=${lit(cancelledLinkedDraft.linked_quality_id)} WHERE actor_id=42 AND bot_id=123456 AND chat_id=42`);
    assert.deepEqual(await cutRow(), cancelledLinkedDraft, 'reverse-link probes restore exactly the terminal row');

    // Replacing a linked draft with a newer direct Cut invalidates the old quality link.
    const replaceQuality = await message(`/video ${url}&replace-linked`);
    const replaceChoice = button(replaceQuality, 'ql:cut:');
    await callback(replaceChoice, replaceQuality);
    const replacedLinked = await cutRow();
    const replacementCutMessage = await message(`/cut ${url}&direct-replacement`);
    const replacementCut = await cutRow();
    assert.notEqual(replacementCut.id, replacedLinked.id);
    const beforeStaleCallback = await cutRow();
    const staleHandoff = await callback(replaceChoice, replaceQuality);
    assert(staleHandoff.ack.data.text || staleHandoff.ack.data.show_alert, 'old qlCut callback cannot transfer replaced draft');
    assert.deepEqual(await cutRow(), beforeStaleCallback, 'stale handoff leaves replacement unchanged');
    const staleQualityLink = await message('/quality');
    assert.match(staleQualityLink.reply.data.text, /This request can no longer open the clip editor\. Send the URL again, or start \/cut URL\./,
      'quality resume gives actionable stale-link guidance');
    assert.equal(buttons(staleQualityLink.reply).length, 0, 'stale linked quality has no full-file controls');
    const qualityAfterStale = await qualityRow();
    assert.equal(qualityAfterStale.stage, 'cutting');
    const terminalCutReplay = await callback(button(replaceQuality, 'ql:cut:'), replaceQuality);
    assert(terminalCutReplay.ack.data.text || terminalCutReplay.ack.data.show_alert);
    assert.deepEqual(await cutRow(), replacementCut, 'replay cannot recreate quality-linked Cut');
    const staleLinkRefresh = await waitFor(() => botApi.calls.slice(terminalCutReplay.from).find(c => c.method === 'editmessagetext'
      && c.data.message_id === replaceQuality.reply.messageId), 'stale linked chooser refresh');
    assert.match(staleLinkRefresh.data.text, /This request can no longer open the clip editor\. Send the URL again, or start \/cut URL\./,
      'rejected callback refresh gives actionable stale-link guidance');
    assert(!buttons(staleLinkRefresh).some(item => item.callback_data?.startsWith('ql:cut:')),
      'rejected callback refresh removes the impossible Resume Cut action');

    // Full-video handoff starts without a seeded range and accepts real interactive times.
    await message('cancel');
    const fullVideo = await message(`/video ${url}&fresh-full-video`);
    const fullCutChoice = button(fullVideo, 'ql:cut:');
    await callback(fullCutChoice, fullVideo);
    const fullStart = await message('/cut');
    assert(/start/i.test(fullStart.reply.data.text));
    const startPrompt = buttons(fullStart.reply).find(item => item.callback_data?.startsWith('cut:edit:'));
    if (startPrompt) await callback(startPrompt.callback_data, fullStart);
    const enteredStart = await message('0.2');
    assert(/end/i.test(enteredStart.reply.data.text));
    const enteredEnd = await message('0.7');
    assert(/clip|summary|confirm/i.test(enteredEnd.reply.data.text));
    assert(buttons(enteredEnd.reply).some(item => item.callback_data?.startsWith('cut:go:')));
    assert.equal(await count(), beforeBlockedOperations, 'fresh full-video handoff remains pre-admission through Ready');
    await message('cancel');

    await assertWorkspaceClean();
    return 'native quality-to-Cut Bot API handoff preserves durable linked draft and frozen request, then delivers real clipped media exactly once';
  } finally {
    botApi.setExpectedUpload(null);
    await fixtureSql(`UPDATE users SET ${userFields.map(f => `${f}=${lit(originalUser[f])}`).join(',')} WHERE id=42`);
    if (originalSettings.trim()) {
      const row = JSON.parse(originalSettings);
      await fixtureSql(`UPDATE dl_user_settings SET ${settingsFields.map(f => `${f}=${lit(row[f])}`).join(',')} WHERE user_id=42`);
    } else await fixtureSql('DELETE FROM dl_user_settings WHERE user_id=42');
    await restore('runtime_cut_sessions', cutFields, JSON.parse(originalCuts));
    await restore('runtime_quality_sessions', qualityFields, JSON.parse(originalQuality));
    assert.equal(await fixtureSql(`SELECT coalesce(json_group_array(json_object(${qualityFields.map(f => `'${f}',${f}`).join(',')})),'[]') FROM runtime_quality_sessions WHERE actor_id=42`), originalQuality, 'quality rows restored including links');
    assert.equal(await fixtureSql(`SELECT coalesce(json_group_array(json_object(${cutFields.map(f => `'${f}',${f}`).join(',')})),'[]') FROM runtime_cut_sessions WHERE actor_id=42`), originalCuts, 'cut rows restored including request_json');
    assert.equal(await fixtureSql(settingsSql(settingsFields)), originalSettings, 'all settings and updated_at restored');
    assert.equal(await fixtureSql(`SELECT json_object(${userFields.map(f => `'${f}',${f}`).join(',')}) FROM users WHERE id=42`), JSON.stringify(originalUser), 'user row restored');
  }
}
