import assert from 'node:assert/strict';
import { commandHarness, admissionNotice } from './command-harness.mjs';
import { withRussianPreference } from './language-fixture.mjs';

const lit = value => value === null ? 'NULL' : typeof value === 'number' ? String(value) : `'${String(value).replaceAll("'", "''")}'`;
const uuid32 = /^[0-9a-f]{32}$/i;
const callbackPattern = /^cut:(add|e(\d{1,2})|d(\d{1,2})|back):([0-9a-f]{32}):(\d+)$/i;

export async function runCutSegmentsScenarios(context) {
  const { botApi, sourcePort, sourceFixture, fixtureSql, waitFor, restartApp, assertWorkspaceClean } = context;
  const h = commandHarness(context, 23000);
  const url = `http://127.0.0.1:${sourcePort}/media?case=cut-segments-${Date.now()}`;
  let nextMessage = 23000;
  let nextCallback = 0;
  const userFields = ['role', 'ban_until', 'language', 'updated_at'];
  const settingsFields = ['quality','codec','container','proxy_enabled','proxy_url','keyboard','subs_enabled','subs_auto','subs_always_ask','subs_lang','split_size','nsfw_blur','mediainfo','send_as_file','audio_codec','gallery_zip','use_pool_cookies','youtube_auth_mode','dm_topic_thread_id','args_json','created_at','updated_at'];
  const fields = async table => (await fixtureSql(`SELECT name FROM pragma_table_info('${table}') ORDER BY cid`)).split('\n').filter(Boolean);
  const object = columns => `json_object(${columns.map(c => `'${c}',${c}`).join(',')})`;
  const originalUserText = (await fixtureSql(`SELECT ${object(userFields)} FROM users WHERE id=42`)).trim();
  const originalUser = JSON.parse(originalUserText);
  const settingsText = (await fixtureSql(`SELECT ${object(settingsFields)} FROM dl_user_settings WHERE user_id=42`)).trim();
  const cutColumns = await fields('runtime_cut_sessions');
  const qualityColumns = await fields('runtime_quality_sessions');
  const cutObject = object(cutColumns);
  const qualityObject = object(qualityColumns);
  const rows = table => fixtureSql(`SELECT coalesce(json_group_array(${table === 'runtime_cut_sessions' ? cutObject : qualityObject}),'[]') FROM ${table} WHERE actor_id=42`);
  const originalCuts = await rows('runtime_cut_sessions');
  const originalQuality = await rows('runtime_quality_sessions');
  const initialOperations = Number(await fixtureSql('SELECT count(*) FROM runtime_operations'));
  const count = async () => Number(await fixtureSql('SELECT count(*) FROM runtime_operations'));
  const settingsNow = () => fixtureSql(`SELECT ${object(settingsFields)} FROM dl_user_settings WHERE user_id=42`);
  const sourceReads = sourceFixture.mediaRequests;
  const buttons = value => {
    const markup = value.reply?.data?.reply_markup ?? value.data?.reply_markup;
    return (typeof markup === 'string' ? JSON.parse(markup) : markup)?.inline_keyboard?.flat() ?? [];
  };
  async function message(text, { id = nextMessage++, language = 'en' } = {}) {
    const from = botApi.calls.length;
    botApi.enqueue(42, 42, text, id, language);
    const reply = await waitFor(() => botApi.calls.slice(from).find(c => c.method === 'sendmessage' && Number(c.data.chat_id) === 42), `multi-cut reply to ${text.split(' ')[0]}`);
    assert(!reply.data.text.includes('cut-segments-'), 'editor cannot echo its source URL');
    return { reply, from, id };
  }
  async function callback(data, origin, options = {}) {
    const from = botApi.calls.length;
    const id = `cut-segments-${++nextCallback}`;
    botApi.enqueueCallback(options.actor ?? 42, 42, origin.reply.messageId, data, { id, ...options });
    const ack = await waitFor(() => botApi.calls.slice(from).find(c => c.method === 'answercallbackquery' && c.data.callback_query_id === id), 'multi-cut callback acknowledgement');
    return { from, ack };
  }
  const button = (origin, prefix) => {
    const found = buttons(origin).find(item => item.callback_data?.startsWith(prefix));
    assert(found, `expected callback ${prefix}`);
    assert(Buffer.byteLength(found.callback_data) <= 64);
    return found.callback_data;
  };
  async function session() {
    const raw = await fixtureSql(`SELECT ${cutObject} FROM runtime_cut_sessions WHERE actor_id=42 AND bot_id=123456 AND chat_id=42`);
    return raw ? JSON.parse(raw) : null;
  }
  async function admit(from) {
    return (await admissionNotice(context, from, { session: 'cut' })).operationId;
  }
  async function addSegment(origin, start, end) {
    const add = await callback(button(origin, 'cut:add:'), origin);
    assert.match((await waitFor(() => botApi.calls.slice(add.from).find(c => c.method === 'editmessagetext' && Number(c.data.message_id) === origin.reply.messageId)?.data.text, 'add segment start prompt')), /start/i);
    await message(start);
    const endPrompt = await message(end);
    assert(/end|summary|ready/i.test(endPrompt.reply.data.text));
    return endPrompt;
  }
  async function inspectSegments(operationId, expected) {
    const request = await h.request(operationId);
    assert.equal(request.clip_range, null, 'multi-range operations cannot use the legacy singleton range');
    assert(Array.isArray(request.params?.clips), 'multi-range request persists its independent ranges in params.clips');
    assert.deepEqual(request.params.clips.map(({ start_seconds, end_seconds }) => ({ start_seconds, end_seconds })), expected);
    return request;
  }
  try {
    assert(cutColumns.some(column => /clip|segment/i.test(column)), 'durable cut storage must expose its clip/segment data column');
    await fixtureSql("INSERT INTO dl_user_settings (user_id,quality,codec,container,proxy_enabled,keyboard,subs_enabled,subs_auto,subs_always_ask,subs_lang,split_size,nsfw_blur,mediainfo,send_as_file,audio_codec,gallery_zip,use_pool_cookies,youtube_auth_mode,args_json,created_at,updated_at) VALUES (42,'ask','avc1','mp4',0,'2x3',0,0,0,'en',2043000000,1,0,0,'best',0,1,'cookies','{}',unixepoch('now')*1000000,unixepoch('now')*1000000) ON CONFLICT(user_id) DO UPDATE SET quality='ask',codec='avc1',container='mp4',send_as_file=0,proxy_enabled=0");
    const qualityRequest = await message(`/video ${url} --send-as-file true --video-codec h264 --container mp4`);
    const cutHandoff = await callback(button(qualityRequest, 'ql:cut:'), qualityRequest);
    assert(!cutHandoff.ack.data.text || !/error/i.test(cutHandoff.ack.data.text), 'quality chooser transfers the request into Cut');
    const empty = await message('/cut');
    assert.match(empty.reply.data.text, /start/i);
    const handoff = await session();
    const frozenOriginal = JSON.parse(handoff.original_request_json);
    assert.equal(frozenOriginal.source_url, url, 'Quality->Cut freezes the original source');
    assert.equal(frozenOriginal.params.engine, 'auto', 'Quality->Cut retains automatic engine intent before admission');
    assert.equal(frozenOriginal.params.send_as_file, false, 'explicit format remains separate from DownloadParams');
    assert.deepEqual(frozenOriginal.preference_overrides, { video_codec: 'h264', audio_codec: null, container: 'mp4', send_as_file: true, gallery_zip: null });
    assert.equal(frozenOriginal.target.reply_to_message_id, qualityRequest.id, 'handoff preserves original delivery target');
    const firstEnd = await message('0.1');
    assert(/end/i.test(firstEnd.reply.data.text));
    const firstReady = await message('0.3');
    const readyActions = buttons(firstReady).map(item => item.callback_data ?? '');
    assert(readyActions.some(data => data.startsWith('cut:add:')), 'Ready exposes Add');
    assert(readyActions.some(data => data.startsWith('cut:go:')), 'Ready exposes Go');
    assert(readyActions.some(data => data.startsWith('cut:edit:')), 'singleton editor retains legacy Edit');
    assert(readyActions.some(data => data.startsWith('cut:d0:')), 'Ready exposes deletion for its committed range');
    assert.match(firstReady.reply.data.text, /1[.)]?[ \t].*(0\.1|00:00\.1)/i, 'Ready presents the first numbered range');
    assert.equal(await count(), initialOperations, 'draft entry cannot admit an operation');
    assert.equal(sourceFixture.mediaRequests, sourceReads, 'editor and Ready cannot probe the source');
    const firstSession = await session();
    const clipColumn = cutColumns.find(column => /clip|segment/i.test(column));
    const parseClips = row => {
      const value = row[clipColumn];
      const stored = typeof value === 'string' ? JSON.parse(value) : value;
      return stored.map(([start_seconds, end_seconds]) => ({ start_seconds, end_seconds }));
    }
    const committedFirst = structuredClone(parseClips(firstSession));
    assert(Array.isArray(committedFirst) && committedFirst.length === 1, 'first completed range is durably committed');

    const secondReady = await addSegment(firstReady, '0.5', '0.8');
    assert.match(secondReady.reply.data.text, /2[.)]?[ \t]/, 'Ready summarizes clips in order');
    const withTwo = await session();
    assert.equal(parseClips(withTwo).length, 2);
    const addSelection = button(secondReady, 'cut:add:');
    const currentAction = addSelection.match(callbackPattern);
    assert(currentAction && uuid32.test(currentAction[4]));
    const staleBefore = await callback(addSelection, secondReady, { actor: 43 });
    assert(staleBefore.ack.data.text || staleBefore.ack.data.show_alert, 'foreign owner cannot mutate editor');
    const staleMalformed = await callback(`cut:add:${currentAction[4]}:0`, secondReady);
    assert(staleMalformed.ack.data.text || staleMalformed.ack.data.show_alert, 'zero revision callback is rejected');
    for (const value of [20, 99]) {
      const bad = await callback(`cut:e${value}:${currentAction[4]}:${currentAction[5]}`, secondReady);
      assert(bad.ack.data.text || bad.ack.data.show_alert, `out-of-range edit ${value} is rejected`);
    }
    assert.deepEqual(parseClips(await session()), parseClips(withTwo), 'invalid callbacks preserve committed clips');
    const edit0 = button(secondReady, 'cut:e0:');
    await callback(edit0, secondReady);
    const editedStart = await message('0.2');
    assert(/end/i.test(editedStart.reply.data.text));
    await restartApp();
    assert.deepEqual(parseClips(await session()), parseClips(withTwo), 'pending edit input preserves previously committed ranges across restart');
    const editedEnd = await message('0.4');
    assert(/summary|ready|clip/i.test(editedEnd.reply.data.text));
    const afterEdit = await session();
    const staleRevision = await callback(addSelection, secondReady);
    assert(staleRevision.ack.data.text || staleRevision.ack.data.show_alert, 'previous editor revision is stale after editing');
    assert.deepEqual(parseClips(await session()), parseClips(afterEdit), 'stale revision cannot mutate committed clips');
    assert.deepEqual(parseClips(afterEdit).slice(1), parseClips(withTwo).slice(1), 'editing one range preserves other committed ranges');
    assert.deepEqual(parseClips(afterEdit)[0], { start_seconds: 0.2, end_seconds: 0.4 }, 'completed edit replaces only its selected range');
    const editedReady = await message('/cut');
    await callback(button(editedReady, 'cut:e1:'), editedReady);
    const pendingEdit = await message('0.6');
    const beforeBack = await session();
    const backReply = await callback(button(pendingEdit, 'cut:back:'), pendingEdit);
    assert(!backReply.ack.data.text || !/error/i.test(backReply.ack.data.text));
    const restoredReady = await message('/cut');
    assert.match(restoredReady.reply.data.text, /ready|summary|clip/i);
    assert.deepEqual(parseClips(await session()), parseClips(beforeBack), 'Back abandons pending edit and restores Ready unchanged');
    assert.equal((await session()).stage, 'ready');

    const current = await session();
    const beforeDuplicate = structuredClone(parseClips(current));
    await callback(button(restoredReady, 'cut:e0:'), restoredReady);
    const duplicateMessageId = nextMessage++;
    const prompt = await message('0.2', { id: duplicateMessageId });
    const duplicate = await message('0.2', { id: duplicateMessageId });
    assert.deepEqual(parseClips(await session()), beforeDuplicate, 'duplicate Telegram message cannot erase or replace a committed range');
    assert.equal(prompt.reply.data.text, duplicate.reply.data.text);
    const duplicateEnd = await message('0.4');
    assert.deepEqual(parseClips(await session())[1], beforeDuplicate[1], 'duplicate start message cannot become the end value');
    const currentReady = await message('/cut');

    const deleteSecond = await callback(button(currentReady, 'cut:d1:'), currentReady);
    assert(!deleteSecond.ack.data.text || !/error/i.test(deleteSecond.ack.data.text));
    const oneLeft = await message('/cut');
    assert.deepEqual(parseClips(await session()), [beforeDuplicate[0]], 'delete removes only its selected ordered range');
    const deleteLast = await callback(button(oneLeft, 'cut:d0:'), oneLeft);
    assert(!deleteLast.ack.data.text || !/error/i.test(deleteLast.ack.data.text));
    const emptyAgain = await message('/cut');
    assert.equal((await session()).stage, 'start', 'deleting the last range returns to Start');
    assert(!buttons(emptyAgain).some(item => item.callback_data?.startsWith('cut:go:')), 'empty editor does not expose Go');
    // Rebuild two complete clips for real-worker and cache/replay acceptance.
    await message('0.1');
    await message('0.3');
    const rebuilt = await message('/cut');
    await addSegment(rebuilt, '0.5', '0.8');
    const ready = await message('/cut');
    const clipState = parseClips(await session());
    assert.equal(clipState.length, 2);
    const russian = await withRussianPreference(fixtureSql, () => message('/cut'));
    assert(/[А-Яа-яЁё]/u.test(russian.reply.data.text), 'Ready summary localizes in Russian');
    const russianActions = buttons(russian).map(item => item.callback_data ?? '');
    for (const prefix of ['cut:add:', 'cut:e0:', 'cut:d0:', 'cut:go:']) assert(russianActions.some(data => data.startsWith(prefix)), `Russian Ready retains ${prefix} control`);
    const launch = await callback(button(ready, 'cut:go:'), ready);
    const menu = await waitFor(() => botApi.calls.slice(launch.from).find(c => c.method === 'editmessagetext' && Number(c.data.message_id) === ready.reply.messageId && buttons(c).some(b => b.callback_data?.startsWith('cut:q'))), 'multi-cut quality choices');
    const chooser = { reply: { ...menu, messageId: Number(menu.data.message_id) }, id: ready.id };
    const served = buttons(chooser).filter(item => /^cut:q(?:best|\d+):[0-9a-f]{32}:[1-9]\d*$/.test(item.callback_data ?? ''));
    assert(served.length > 0, 'Go presents quality choices');
    const selection = served.find(item => item.callback_data.startsWith('cut:q720:')) ?? served[0];
    const readsBeforeChoice = sourceFixture.mediaRequests;
    assert.equal(await count(), initialOperations, 'quality menu cannot admit work');
    assert.equal(sourceFixture.mediaRequests, sourceReads, 'quality menu cannot contact source');
    const preferencesBeforeConfirm = await settingsNow();
    for (const invalid of [
      { actor: 43 }, { isBot: true }, { inaccessible: true },
      { messagePatch: { message_thread_id: 9 } },
      { messagePatch: { chat: { id: -100, type: 'group', title: 'Foreign' } } },
      { messagePatch: { chat: { id: 43, type: 'private', first_name: 'Foreign' } } },
      { messagePatch: { from: { id: 999, is_bot: true, first_name: 'Foreign bot' } } },
    ]) {
      const denied = await callback(selection.callback_data, chooser, invalid);
      assert(denied.ack.data.text || denied.ack.data.show_alert, 'foreign callback context is rejected');
    }
    await fixtureSql("UPDATE users SET role='banned' WHERE id=42");
    const roleDenied = await callback(selection.callback_data, chooser);
    assert(roleDenied.ack.data.text || roleDenied.ack.data.show_alert, 'banned role cannot confirm');
    await fixtureSql("UPDATE users SET role='owner',ban_until=unixepoch('now')*1000000+600000000 WHERE id=42");
    const banDenied = await callback(selection.callback_data, chooser);
    assert(banDenied.ack.data.text || banDenied.ack.data.show_alert, 'active ban cannot confirm');
    await fixtureSql(`UPDATE users SET role=${lit(originalUser.role)},ban_until=${lit(originalUser.ban_until)} WHERE id=42`);
    assert.equal(await count(), initialOperations, 'denied callbacks cannot admit work');
    assert.equal(sourceFixture.mediaRequests, readsBeforeChoice, 'denied callbacks cannot contact source');
    assert.equal(await settingsNow(), preferencesBeforeConfirm, 'editor and quality choice cannot mutate preferences');
    botApi.setExpectedAlbum(clipState.map(() => ({ kind: 'document', filename: 'media.mp4', mimeType: 'video/mp4' })));
    const selected = await callback(selection.callback_data, chooser);
    const operationId = await admit(selected.from);
    await h.succeeded(operationId);
    const operation = await inspectSegments(operationId, clipState);
    assert(operation.params.clips.length >= 2);
    assert.equal(operation.params.max_height, selection.callback_data.startsWith('cut:q720:') ? 720 : operation.params.max_height);
    assert.equal(operation.params.send_as_file, true, 'Quality->Cut retains explicit file delivery override');
    assert.equal(operation.params.video_codec, 'h264', 'Quality->Cut retains explicit video codec');
    assert.equal(operation.params.container, 'mp4', 'Quality->Cut retains explicit container');
    assert.equal(operation.params.engine, 'auto_yt_dlp_v1', 'admission freezes automatic engine intent to the fixture yt-dlp strategy');
    assert.equal(operation.target.reply_to_message_id, qualityRequest.id, 'multi-cut delivery keeps the original Quality request target');
    assert.equal(await settingsNow(), preferencesBeforeConfirm, 'confirmation and Quality->Cut preserve actor preferences');
    assert(sourceFixture.mediaRequests > readsBeforeChoice, 'confirmed operation contacts source for worker processing');
    assert.equal(botApi.lastGroupBytes.length, clipState.length, 'each range produces its own ordered file');
    for (const [index, bytes] of botApi.lastGroupBytes.entries()) {
      const probe = JSON.parse(await context.helperContainer([
        '--network', 'none', '--entrypoint', 'ffprobe', context.image,
        '-v', 'error', '-show_entries', 'format=duration:stream=codec_type', '-of', 'json', '-i', 'pipe:0',
      ], 15000, bytes));
      const duration = Number(probe.format.duration);
      const expected = clipState[index].end_seconds - clipState[index].start_seconds;
      assert(Math.abs(duration - expected) <= 0.06, `segment ${index + 1} duration ${duration} must match ${expected}`);
      assert(probe.streams.some(stream => stream.codec_type === 'video'));
      assert(probe.streams.some(stream => stream.codec_type === 'audio'));
    }
    const admittedCount = await count();
    const sends = botApi.sendCalls;
    const uploads = botApi.uploadCalls;
    const frozen = structuredClone(operation);
    const completedSourceRequests = sourceFixture.mediaRequests;
    await fixtureSql("UPDATE runtime_cut_sessions SET stage='confirming',operation_id=NULL WHERE actor_id=42 AND bot_id=123456 AND chat_id=42");
    await restartApp();
    const replayMenu = await message('/cut');
    const recoveredButton = button(replayMenu, `cut:q${selection.callback_data.split(':')[1].slice(1)}:`);
    const replay = await callback(recoveredButton, replayMenu);
    assert.equal(await admit(replay.from), operationId, 'same selected height recovers the existing operation');
    assert.equal(await count(), admittedCount);
    assert.equal(botApi.sendCalls, sends, 'same-choice recovery does not deliver a second time');
    assert.equal(botApi.uploadCalls, uploads);
    assert.equal(sourceFixture.mediaRequests, completedSourceRequests, 'recovery makes no new source requests');
    assert.deepEqual(await h.request(operationId), frozen, 'recovery preserves frozen clips and quality');
    const outputs = botApi.calls.filter(call => call.method === 'sendmediagroup');
    assert(outputs.length >= 1);
    botApi.setExpectedUpload(null);

    const cachedSourceRequests = sourceFixture.mediaRequests;
    const cachedSends = botApi.cachedCalls;
    const cachedUploads = botApi.uploadCalls;
    const cachedQualityRequest = await message(`/video ${url} --send-as-file true --video-codec h264 --container mp4`);
    await callback(button(cachedQualityRequest, 'ql:cut:'), cachedQualityRequest);
    const cachedStart = await message('/cut');
    assert.match(cachedStart.reply.data.text, /start/i);
    await message('0.1');
    const cachedReady = await message('0.3');
    await addSegment(cachedReady, '0.5', '0.8');
    const cachedDraft = await message('/cut');
    const cachedGo = await callback(button(cachedDraft, 'cut:go:'), cachedDraft);
    const cachedMenuCall = await waitFor(() => botApi.calls.slice(cachedGo.from).find(c => c.method === 'editmessagetext'
      && Number(c.data.message_id) === cachedDraft.reply.messageId && buttons(c).some(b => b.callback_data?.startsWith('cut:q720:'))), 'cached multi-cut quality menu');
    const cachedMenu = { reply: { ...cachedMenuCall, messageId: Number(cachedMenuCall.data.message_id) }, id: cachedDraft.id };
    const cachedChoice = await callback(button(cachedMenu, 'cut:q720:'), cachedMenu);
    const cachedOperationId = await admit(cachedChoice.from);
    await h.succeeded(cachedOperationId);
    assert.notEqual(cachedOperationId, operationId, 'a new user request receives its own operation identity');
    assert.equal(botApi.cachedCalls, cachedSends + 1, 'identical multi-range request reuses confirmed Telegram cache after restart');
    assert.equal(botApi.uploadCalls, cachedUploads, 'cache replay does not upload media again');
    assert.equal(sourceFixture.mediaRequests, cachedSourceRequests, 'cache replay after restart does not repeat source access');
    assert.deepEqual((await inspectSegments(cachedOperationId, clipState)).params.clips, frozen.params.clips);
    assert.equal(await count(), admittedCount + 1, 'cache replay creates only one new operation');
    await assertWorkspaceClean();
  } finally {
    botApi.setExpectedUpload(null);
    botApi.setExpectedAlbum(null);
    await fixtureSql(`UPDATE users SET ${userFields.map(field => `${field}=${lit(originalUser[field])}`).join(',')} WHERE id=42`);
    if (settingsText) {
      const saved = JSON.parse(settingsText);
      await fixtureSql(`UPDATE dl_user_settings SET ${settingsFields.map(field => `${field}=${lit(saved[field])}`).join(',')} WHERE user_id=42`);
    } else await fixtureSql('DELETE FROM dl_user_settings WHERE user_id=42');
    for (const [table, columns, snapshot] of [['runtime_cut_sessions', cutColumns, originalCuts], ['runtime_quality_sessions', qualityColumns, originalQuality]]) {
      await fixtureSql(`DELETE FROM ${table} WHERE actor_id=42`);
      for (const row of JSON.parse(snapshot)) await fixtureSql(`INSERT INTO ${table} (${columns.join(',')}) VALUES (${columns.map(column => lit(row[column])).join(',')})`);
    }
    assert.equal(await rows('runtime_cut_sessions'), originalCuts, 'Cut draft rows restored byte-for-byte');
    assert.equal(await rows('runtime_quality_sessions'), originalQuality, 'quality rows restored byte-for-byte');
    assert.equal((await fixtureSql(`SELECT ${object(userFields)} FROM users WHERE id=42`)).trim(), originalUserText, 'actor state restored');
    assert.equal(await settingsNow(), settingsText, 'actor preferences restored');
  }
  return 'durable multi-segment Bot API editing, recovery, authorization, frozen quality, and real worker delivery verified';
}
