import assert from 'node:assert/strict';
import { setTimeout as sleep } from 'node:timers/promises';
import { commandHarness, admissionNotice } from './command-harness.mjs';
import { withRussianPreference } from './language-fixture.mjs';

export async function runPlainUrlScenarios(context) {
  const { botApi, sourcePort, fixtureSql, waitFor, restartApp, assertWorkspaceClean } = context;
  const h = commandHarness(context, 20000);
  const source = `http://127.0.0.1:${sourcePort}/media?case=plain-url-private-marker`;
  const entity = (text, url = source) => ({ entities: [{ type: 'url', offset: text.indexOf(url), length: url.length }] });
  const count = () => fixtureSql('SELECT count(*) FROM runtime_operations');
  const sends = () => botApi.calls.filter(call => call.method === 'sendmessage');
  let nextId = 21000;
  async function reply(text, options = {}, prefix) {
    const from = botApi.calls.length;
    botApi.enqueue(42, 42, text, nextId++, options.language ?? 'en', options);
    return waitFor(() => botApi.calls.slice(from).find(call => call.method === 'sendmessage'
      && Number(call.data.chat_id) === 42 && (!prefix || call.data.text.startsWith(prefix))), 'plain URL response');
  }
  const sendAsFile = (await fixtureSql('SELECT COALESCE((SELECT send_as_file FROM dl_user_settings WHERE user_id=42),0)')) === '1';
  const method = sendAsFile ? 'senddocument' : 'sendvideo';
  const contract = { filename: 'media.mp4', mimeType: 'video/mp4' };
  try {
    const text = `🦀 Видео ${source}`;
    const first = await h.execute('visible URL with UTF-16 prefix', text, method,
      { video: true, audio: true, durationMin: 0.8, durationMax: 1.5 }, contract, entity(text));
    const frozen = await h.request(first.operationId);
    assert.equal(frozen.source_url, source);
    assert.equal(frozen.params.send_as_file, sendAsFile, 'plain URL inherits saved delivery preferences');
    assert.equal(frozen.target.reply_to_message_id, first.messageId);
    await restartApp();
    await h.replay(text, first, entity(text));
    assert.deepEqual(await h.request(first.operationId), frozen);
    // Same update identity with changed URL intent must not become a second job.
    const changed = `${source}&changed=true`;
    const beforeConflict = await count();
    const from = botApi.calls.length;
    botApi.enqueue(42, 42, changed, first.messageId, 'en', entity(changed, changed));
    await waitFor(() => botApi.calls.slice(from).some(call => call.method === 'sendmessage'), 'changed URL replay conflict');
    assert.equal(await count(), beforeConflict);

    const clippedText = `${source} 0.2 0.4`;
    const clip = await h.execute('bare URL duration clip', clippedText, method,
      { video: true, audio: true, durationMin: 0.3, durationMax: 0.75 }, contract, entity(clippedText));
    const clipRange = (await h.request(clip.operationId)).clip_range;
    assert.equal(clipRange.start_seconds, 0.2);
    assert(Math.abs(clipRange.end_seconds - 0.6) < 1e-9);
    assert.equal((await h.request(clip.operationId)).params.playlist ?? null, null);

    const countBeforeInvalid = await count();
    await reply(`${source} 7`, entity(`${source} 7`), 'Send one visible');
    await withRussianPreference(fixtureSql, () => reply(`${source} bad`, { ...entity(`${source} bad`), language: 'en' }, 'Отправьте одну'));
    const malformed = entity(`🦀 ${source}`);
    malformed.entities[0].offset = 1;
    await reply(`🦀 ${source}`, malformed, 'Send one visible');
    assert.equal(await count(), countBeforeInvalid);
    const beforeSilent = sends().length;
    for (const [actor, chat, text, options] of [
      [42, 42, source, {}],
      [42, 42, 'Hidden link', { entities: [{ type: 'text_link', offset: 0, length: 11, url: source }] }],
      [42, 42, `/unknown ${source}`, entity(`/unknown ${source}`)],
      [42, 42, `/video@other_bot ${source}`, entity(`/video@other_bot ${source}`)],
      [42, 42, source, { ...entity(source), viaBot: 999 }],
      [43, 44, source, entity(source)],
      [42, -10042, source, { ...entity(source), replyToBot: 123456 }],
      [42, 42, 'https://open.spotify.com/track/test', entity('https://open.spotify.com/track/test', 'https://open.spotify.com/track/test')],
    ]) botApi.enqueue(actor, chat, text, nextId++, 'en', options);
    await reply('/help', {}, 'Available:');
    await sleep(300);
    assert.equal(sends().length, beforeSilent + 1, 'non-download messages and unauthorized routes remain silent');
    assert.equal(await count(), countBeforeInvalid);
    await fixtureSql("UPDATE users SET ban_until=unixepoch('now')*1000000+600000000 WHERE id=42");
    await reply(source, entity(source));
    assert.equal(await count(), countBeforeInvalid, 'ban blocks plain URL admission');
    await fixtureSql('UPDATE users SET ban_until=NULL WHERE id=42');

    // A URL timestamp enters the persisted private clip dialog, not a whole-video job.
    const timestamped = `${source}&t=0.2`;
    await reply(timestamped, entity(timestamped, timestamped), 'Clip end');
    assert.equal(await count(), countBeforeInvalid);
    await restartApp();
    await reply('/cut', {}, 'Clip end');
    const summary = await reply('0.7', {}, 'Clip summary');
    const markup = typeof summary.data.reply_markup === 'string' ? JSON.parse(summary.data.reply_markup) : summary.data.reply_markup;
    const go = markup.inline_keyboard.flat().find(button => button.callback_data.startsWith('cut:go:'));
    assert(go);
    botApi.setExpectedUpload({ method, preserveMdat: false, ...contract });
    const beforeGo = botApi.calls.length;
    botApi.enqueueCallback(42, 42, summary.messageId, go.callback_data);
    const { operationId: id } = await admissionNotice(context, beforeGo);
    await h.succeeded(id);
    assert.deepEqual((await h.request(id)).clip_range, { start_seconds: 0.2, end_seconds: 0.7 });
    await h.inspectUpload('URL timestamp dialog', { video: true, audio: true, durationMin: 0.35, durationMax: 0.8 });
    assert(botApi.calls.filter(call => ['sendmessage', 'editmessagetext'].includes(call.method))
      .every(call => !call.data.text.includes('plain-url-private-marker')), 'source URL must not appear in bot replies');
    await assertWorkspaceClean();
  } finally {
    botApi.setExpectedUpload(null);
    await fixtureSql('UPDATE users SET ban_until=NULL WHERE id=42');
  }
  return 'visible UTF-16 URL entities, saved preferences, real full/clipped output, restart replay/conflict, timestamp dialog, silent exclusions and authorization passed';
}
