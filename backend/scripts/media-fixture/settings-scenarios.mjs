import assert from 'node:assert/strict';
import { commandHarness } from './command-harness.mjs';
import { withRussianPreference } from './language-fixture.mjs';

export async function runSettingsScenarios(context) {
  const { botApi, fixtureSql, sourcePort, waitFor, restartApp } = context;
  const h = commandHarness(context, 5000);
  const fields = ['quality', 'codec', 'container', 'audio_codec', 'send_as_file', 'gallery_zip', 'subs_lang', 'subs_auto', 'nsfw_blur', 'updated_at'];
  const snapshot = () => fixtureSql(`SELECT json_object(${fields.map(f => `'${f}',${f}`).join(',')}) FROM dl_user_settings WHERE user_id=42`);
  const old = await snapshot();
  const role = await fixtureSql('SELECT role FROM users WHERE id=42');
  let next = 5100;
  async function command(text, language = 'en', prefix = 'Download settings') {
    const start = botApi.calls.length;
    botApi.enqueue(42, 42, text, next++, language);
    return waitFor(() => botApi.calls.slice(start).find(c => c.method === 'sendmessage' && c.data.text.startsWith(prefix)), `settings response ${text.split(' ')[0]}`);
  }
  async function click(menu, data, options = {}, edit = true, actor = 42) {
    if (data.split(':').length === 3 && !data.endsWith(':_menu')) {
      if (edit) {
        await click(menu, `prefs:${data.split(':')[1]}:_menu`);
        const current = botApi.calls.filter(c => c.method === 'editmessagetext' && c.data.message_id === menu.messageId).at(-1);
        const button = current.data.reply_markup.inline_keyboard.flat().find(b => b.callback_data.startsWith(`${data}:`));
        assert(button, 'value must come from an actual served button');
        data = button.callback_data;
      } else data += `:${await fixtureSql('SELECT coalesce((SELECT updated_at FROM dl_user_settings WHERE user_id=42),0)')}`;
    }
    const start = botApi.calls.length;
    const id = botApi.enqueueCallback(actor, 42, menu.messageId, data, options);
    const ack = await waitFor(() => botApi.calls.slice(start).find(c => c.method === 'answercallbackquery' && c.data.callback_query_id === id), 'callback acknowledgement');
    if (edit) await waitFor(() => botApi.calls.slice(start).find(c => c.method === 'editmessagetext' && c.data.message_id === menu.messageId), 'settings menu edit');
    assert.equal(botApi.calls.slice(start).filter(c => c.method === 'answercallbackquery' && c.data.callback_query_id === id).length, 1);
    return { ...ack, callbackData: data };
  }
  try {
    const operations = await fixtureSql('SELECT count(*) FROM runtime_operations');
    let menu = await command('/settings');
    assert.deepEqual(menu.data.reply_markup.inline_keyboard.map(row => row[0].callback_data).sort(),
      fields.filter(field => field !== 'updated_at').map(field => `prefs:${field}:_menu`).sort());
    assert(!JSON.stringify(menu).includes('proxy_url'));
    await click(menu, 'prefs:quality:_menu');
    const submenu = botApi.calls.filter(c => c.method === 'editmessagetext' && c.data.message_id === menu.messageId).at(-1);
    assert(submenu.data.reply_markup.inline_keyboard.flat().some(b => b.callback_data.startsWith('prefs:quality:360:')));
    await click(menu, 'prefs:quality:360');
    const first = await click(menu, 'prefs:send_as_file:true', { id: 'replayed-settings' });
    await click(menu, 'prefs:send_as_file:false');
    const stale = await click(menu, first.callbackData, { id: 'replayed-settings' }, false);
    assert.equal(stale.data.show_alert, true);
    assert.equal(JSON.parse(await snapshot()).send_as_file, 0);
    await click(menu, 'prefs:send_as_file:true');
    assert.equal(JSON.parse(await snapshot()).send_as_file, 1);
    await command('/settings subs_lang zh-Hans');
    assert.equal(JSON.parse(await snapshot()).subs_lang, 'zh-Hans');
    await command('/settings gallery_zip true');
    await command('/settings subs_auto true');
    assert.equal(JSON.parse(await snapshot()).gallery_zip, 1);
    assert.equal(JSON.parse(await snapshot()).subs_auto, 1);
    let stable = await snapshot();
    for (const data of ['prefs:quality:bogus', 'prefs:role:owner', 'prefs:audio_codec:secret:extra']) await click(menu, data, {}, false);
    for (const options of [{ isBot: true }, { inaccessible: true }, { messagePatch: { message_thread_id: 9 } },
      { messagePatch: { chat: { id: -42, type: 'group', title: 'Fixture' } } },
      { messagePatch: { from: { id: 999, is_bot: true, first_name: 'Other bot' } } }]) {
      await click(menu, 'prefs:quality:720', options, false);
    }
    await click(menu, 'prefs:quality:720', {}, false, 43);
    await command('/settings proxy_url secret-marker', 'en', 'Use /settings');
    assert.equal(await snapshot(), stable, 'invalid/cross-actor/cross-chat callbacks cannot mutate settings');
    await fixtureSql("UPDATE users SET role='restricted' WHERE id=42");
    await click(menu, 'prefs:quality:360');
    assert.equal(JSON.parse(await snapshot()).quality, '360', 'configured owner retains elevation when not globally blocked');
    stable = await snapshot();
    await fixtureSql("UPDATE users SET role='banned' WHERE id=42");
    const denied = await click(menu, 'prefs:quality:720', {}, false);
    assert.equal(denied.data.show_alert, true);
    assert.equal(await snapshot(), stable);
    await fixtureSql(`UPDATE users SET role='${role}' WHERE id=42`);
    await restartApp();
    menu = await withRussianPreference(fixtureSql, () => command('/format@yoink_fixture_bot', 'en', 'Настройки загрузки'));
    assert(menu.data.text.includes('360'));
    assert(menu.data.text.includes('zh-Hans'));
    assert.equal(await snapshot(), stable);
    assert.equal(await fixtureSql('SELECT count(*) FROM runtime_operations'), operations);
    // Change all encoding fields through the user-facing path, not fixture SQL.
    await command('/settings codec avc1');
    await command('/settings container mp4');
    const video = await h.execute('saved document delivery', `/video http://127.0.0.1:${sourcePort}/media.mp4?case=settings-video`, 'senddocument',
      { video: true, audio: true, height: 120 }, { filename: 'media.mp4', mimeType: 'video/mp4' });
    const frozen = await h.request(video.operationId);
    assert.equal(frozen.params.max_height, 360);
    assert.equal(frozen.params.send_as_file, true);
    await command('/settings quality 720');
    await h.replay(`/video http://127.0.0.1:${sourcePort}/media.mp4?case=settings-video`, video);
    assert.deepEqual(await h.request(video.operationId), frozen);
    await command('/settings send_as_file false');
    await command('/settings audio_codec mp3');
    await h.execute('saved MP3 audio', `/audio http://127.0.0.1:${sourcePort}/media.mp4?case=settings-audio`, 'sendaudio',
      { video: false, audio: true, audioCodec: 'mp3' }, { filename: 'audio.mp3', mimeType: 'audio/mpeg' });
    assert.equal(botApi.error, undefined);
  } finally {
    await fixtureSql(`UPDATE users SET role='${role}' WHERE id=42`);
    if (old.trim()) {
      const values = JSON.parse(old);
      const literal = v => typeof v === 'number' ? String(v) : `'${v.replaceAll("'", "''")}'`;
      await fixtureSql(`UPDATE dl_user_settings SET ${fields.map(f => `${f}=${literal(values[f])}`).join(',')} WHERE user_id=42`);
    } else await fixtureSql('DELETE FROM dl_user_settings WHERE user_id=42');
    assert.equal(await snapshot(), old, 'settings cleanup must restore values and revision');
    botApi.setExpectedUpload(null);
  }
  return 'private settings commands/buttons, exact-value callback replay, ban/cross-actor/chat denials, EN/RU, restart persistence and actual saved document/MP3 delivery passed';
}
