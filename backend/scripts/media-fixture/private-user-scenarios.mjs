import assert from 'node:assert/strict';
import { commandHarness, admissionNotice } from './command-harness.mjs';
import { parseFormatReport } from './format-scenarios.mjs';

export async function runPrivateUserScenarios(context) {
  const { botApi, fixtureSql, sourcePort, waitFor, restartApp } = context;
  const actor = 43;
  const h = commandHarness(context, 31000);
  let next = 30000;
  const row = await fixtureSql('SELECT json_object(\'role\',role,\'ban_until\',ban_until) FROM users WHERE id=43');
  const ownerSettings = await fixtureSql('SELECT coalesce(json_group_array(json_object(\'revision\',updated_at,\'quality\',quality,\'proxy\',proxy_url,\'args\',args_json)),\'[]\') FROM dl_user_settings WHERE user_id=42');
  const saved = await fixtureSql('SELECT coalesce(json_group_array(json_object(\'quality\',quality,\'codec\',codec,\'container\',container,\'audio_codec\',audio_codec,\'send_as_file\',send_as_file,\'gallery_zip\',gallery_zip,\'subs_lang\',subs_lang,\'subs_auto\',subs_auto,\'nsfw_blur\',nsfw_blur,\'updated_at\',updated_at)),\'[]\') FROM dl_user_settings WHERE user_id=43');
  const literal = v => v === null ? 'NULL' : typeof v === 'number' ? String(v) : `'${String(v).replaceAll("'", "''")}'`;
  const buttons = call => call.data.reply_markup.inline_keyboard.flat();
  async function message(text, prefix) {
    const start = botApi.calls.length;
    botApi.enqueue(actor, actor, text, next++);
    return waitFor(() => botApi.calls.slice(start).find(c => c.method === 'sendmessage' && Number(c.data.chat_id) === actor && (!prefix || c.data.text.startsWith(prefix))), `ordinary-user ${text.split(' ')[0]}`);
  }
  async function click(menu, data, { from = actor, options = {}, edit = false } = {}) {
    const start = botApi.calls.length;
    const id = botApi.enqueueCallback(from, actor, menu.messageId, data, options);
    const ack = await waitFor(() => botApi.calls.slice(start).find(c => c.method === 'answercallbackquery' && c.data.callback_query_id === id), 'ordinary-user callback ack');
    const update = edit ? await waitFor(() => botApi.calls.slice(start).find(c => c.method === 'editmessagetext' && Number(c.data.chat_id) === actor), 'ordinary-user callback edit') : undefined;
    return { ack, update, start };
  }
  try {
    await fixtureSql("UPDATE users SET role='user',ban_until=NULL WHERE id=43");
    const menu = await message('/settings', 'Download settings');
    assert.equal(buttons(menu).length, 15);
    assert.deepEqual(buttons(menu).map(button => button.callback_data.split(':')[1]).sort(), [
      'audio_codec', 'codec', 'container', 'gallery_zip', 'keyboard', 'mediainfo',
      'nsfw_blur', 'quality', 'send_as_file', 'split_size', 'subs_always_ask',
      'subs_auto', 'subs_enabled', 'subs_lang', 'use_pool_cookies',
    ].sort());
    const page = await click(menu, 'prefs:quality:_menu', { edit: true });
    const selected = buttons(page.update).find(b => b.callback_data.startsWith('prefs:quality:360:')).callback_data;
    await click(menu, selected, { edit: true });
    const stale = await click(menu, selected);
    assert.equal(stale.ack.data.show_alert, true);
    await click(menu, 'prefs:quality:_menu', { edit: true });
    const rev = await fixtureSql('SELECT updated_at FROM dl_user_settings WHERE user_id=43');
    for (const options of [{ messagePatch: { from: { id: 999, is_bot: true, first_name: 'Other' } } }, { messagePatch: { message_thread_id: 9 } }, { messagePatch: { chat: { id: 42, type: 'private', first_name: 'Owner' } } }]) {
      await click(menu, `prefs:quality:720:${rev}`, { options });
    }
    await click(menu, `prefs:quality:720:${rev}`, { from: 42 });
    assert.equal(await fixtureSql('SELECT quality FROM dl_user_settings WHERE user_id=43'), '360');
    await message('/format codec avc1', 'Download settings');
    await message('/settings container mp4', 'Download settings');
    await message('/settings send_as_file false', 'Download settings');
    await message('/settings audio_codec best', 'Download settings');
    const delivered = await h.execute('ordinary-user saved format', `/video http://127.0.0.1:${sourcePort}/media.mp4?case=user-format`, 'sendvideo', { video: true, audio: true, height: 120 }, {}, { actor, chat: actor, messageId: next++ });
    assert.equal((await h.request(delivered.operationId)).params.max_height, 360);
    botApi.setExpectedUpload({ method: 'senddocument', filename: 'formats.txt', mimeType: 'text/plain', preserveMdat: false });
    const formats = await h.submit(`/list http://127.0.0.1:${sourcePort}/media.mp4?case=user-formats`, { actor, chat: actor, messageId: next++ });
    await h.succeeded(formats.operationId);
    parseFormatReport(botApi.lastUploadedBytes);
    botApi.setExpectedUpload(null);
    await message('/audio');
    const deferredUrl = `http://127.0.0.1:${sourcePort}/media.mp4?case=user-deferred`;
    const deferredOptions = { actor, chat: actor, messageId: next++, entities: [{ type: 'url', offset: 0, length: deferredUrl.length }] };
    const deferred = await h.execute('ordinary-user deferred audio', deferredUrl, 'sendaudio', { video: false, audio: true }, { filename: 'audio.m4a', mimeType: 'audio/mp4' }, deferredOptions);
    await h.replay(deferredUrl, deferred, deferredOptions);
    const subs = await message('/subs');
    const subtitleLanguage = buttons(subs).find(b => b.callback_data.startsWith('subs:lang:en:')).callback_data;
    await click(subs, subtitleLanguage, { edit: true });
    await message('/settings quality ask', 'Download settings');
    await message('/nsfw mark');
    const draft = await message(`/video http://127.0.0.1:${sourcePort}/media.mp4?case=user-ask-cut`);
    const cutData = buttons(draft).find(b => b.callback_data.startsWith('ql:cut:')).callback_data;
    await restartApp();
    await click(draft, cutData);
    await message('0.2');
    const ready = await message('0.7');
    const go = buttons(ready).find(b => b.callback_data.startsWith('cut:go:')).callback_data;
    const result = await click(ready, go, { edit: true });
    const chooser = { ...result.update, messageId: ready.messageId };
    assert(buttons(chooser).some(b => b.callback_data.startsWith('cut:q720:')));
    const choice = buttons(chooser).find(b => b.callback_data.startsWith('cut:q720:')).callback_data;
    botApi.setExpectedUpload({ method: 'sendvideo', filename: 'media.mp4', mimeType: 'video/mp4', preserveMdat: false });
    const admitted = await click(chooser, choice);
    const { operationId: id } = await admissionNotice(context, admitted.start);
    await h.succeeded(id);
    await h.inspectUpload('ordinary-user ask cut', { video: true, audio: true, durationMin: 0.3, durationMax: 0.8 });
    const frozen = await h.request(id);
    assert.equal(frozen.actor_id, actor);
    assert.equal(frozen.force_nsfw, true);
    assert.equal(frozen.params.max_height, 720);
    await restartApp();
    await click(chooser, choice);
    assert.deepEqual(await h.request(id), frozen);
    for (const role of ['restricted', 'banned']) {
      await fixtureSql(`UPDATE users SET role='${role}' WHERE id=43`);
      assert.equal((await click(menu, `prefs:quality:720:${await fixtureSql('SELECT updated_at FROM dl_user_settings WHERE user_id=43')}`)).ack.data.show_alert, true);
      assert.equal((await click(chooser, choice)).ack.data.show_alert, true);
    }
    await fixtureSql("UPDATE users SET role='user' WHERE id=43");
    const before = await fixtureSql('SELECT count(*) FROM runtime_operations');
    const start = botApi.calls.length;
    for (const text of ['/status', '/proxy', '/cookie', '/uncache http://example.invalid']) botApi.enqueue(actor, actor, text, next++);
    await message('/settings', 'Download settings');
    assert.equal(await fixtureSql('SELECT count(*) FROM runtime_operations'), before);
    assert(!botApi.calls.slice(start).some(c => ['senddocument', 'sendaudio', 'sendvideo'].includes(c.method)));
    assert.equal(await fixtureSql('SELECT coalesce(json_group_array(json_object(\'revision\',updated_at,\'quality\',quality,\'proxy\',proxy_url,\'args\',args_json)),\'[]\') FROM dl_user_settings WHERE user_id=42'), ownerSettings);
  } finally {
    const user = JSON.parse(row);
    await fixtureSql(`UPDATE users SET role=${literal(user.role)},ban_until=${literal(user.ban_until)} WHERE id=43`);
    const previous = JSON.parse(saved)[0];
    if (previous) await fixtureSql(`UPDATE dl_user_settings SET ${Object.entries(previous).map(([k, v]) => `${k}=${literal(v)}`).join(',')} WHERE user_id=43`);
    else await fixtureSql('DELETE FROM dl_user_settings WHERE user_id=43');
    botApi.setExpectedUpload(null);
  }
  return 'ordinary-user real settings/format, stale and cross-user callbacks, saved ask -> quality -> cut -> admission, restart replay and role/ban revocation';
}
