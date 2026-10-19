import assert from 'node:assert/strict';
import { commandHarness, admissionNotice } from './command-harness.mjs';
import { withRussianPreference } from './language-fixture.mjs';

export async function runCutScenarios(context) {
  const { botApi, sourcePort, fixtureSql, waitFor, restartApp, assertWorkspaceClean } = context;
  const h = commandHarness(context);
  const url = `http://127.0.0.1:${sourcePort}/media?case=cut-dialog-private-marker`;
  let nextMessage = 7000;
  let nextCallback = 0;
  const count = () => fixtureSql('SELECT count(*) FROM runtime_operations');
  const buttons = reply => {
    const markup = reply.data.reply_markup;
    return (typeof markup === 'string' ? JSON.parse(markup) : markup)?.inline_keyboard?.flat() ?? [];
  };
  function button(reply, action) {
    const result = buttons(reply).find(item => item.callback_data?.startsWith(`cut:${action}:`));
    assert(result, `clip dialog must serve a ${action} button`);
    assert(Buffer.byteLength(result.callback_data) <= 64);
    assert(Number.isInteger(reply.messageId) && reply.messageId > 0);
    return { data: result.callback_data, messageId: reply.messageId };
  }
  async function message(text, { messageId = nextMessage++, language = 'en', prefix } = {}) {
    const from = botApi.calls.length;
    botApi.enqueue(42, 42, text, messageId, language);
    const reply = await waitFor(() => botApi.calls.slice(from).find(call =>
      call.method === 'sendmessage' && Number(call.data.chat_id) === 42), 'cut dialog response');
    if (prefix) assert(reply.data.text.startsWith(prefix), `expected ${prefix}, received ${reply.data.text}`);
    assert(!reply.data.text.includes('cut-dialog-private-marker'), 'dialog must not echo source URLs');
    return { reply, from, messageId };
  }
  async function callback(selection, { actor = 42, language = 'en', ...options } = {}) {
    const id = `cut-dialog-${++nextCallback}`;
    const from = botApi.calls.length;
    botApi.enqueueCallback(actor, 42, selection.messageId, selection.data, { id, language, ...options });
    const answer = await waitFor(() => botApi.calls.slice(from).find(call =>
      call.method === 'answercallbackquery' && call.data.callback_query_id === id), 'cut callback acknowledgement');
    return { answer, from };
  }
  async function response(from, prefix) {
    return waitFor(() => botApi.calls.slice(from).find(call => ['sendmessage', 'editmessagetext'].includes(call.method)
      && (!prefix || call.data.text.startsWith(prefix))), 'cut callback response');
  }
  async function confirm(data) {
    const sent = await callback(data);
    const { operationId } = await admissionNotice(context, sent.from, { session: 'cut' });
    return { ...sent, operationId };
  }

  const initialCount = await count();
  try {
    await message(`/cut ${url}`, { prefix: 'Clip start' });
    await message('not-a-time');
    const start = await message('0.2', { prefix: 'Clip end' });
    await restartApp();
    await message('/cut', { prefix: 'Clip end' });
    // The same start message after restart must not become the end value.
    await message('0.2', { messageId: start.messageId, prefix: 'Clip end' });
    await message('00:00.1');
    const ready = await message('0.7', { prefix: 'Clip summary' });
    const oldGo = button(ready.reply, 'go');
    assert.equal(await count(), initialCount, 'drafts and invalid times must not admit jobs');

    const edited = await callback(button(ready.reply, 'edit'));
    await response(edited.from, 'Clip start');
    const stale = await callback(oldGo);
    assert(stale.answer.data.text, 'old confirmation must receive a stale-state explanation');
    assert.equal(await count(), initialCount);
    await message('0.2', { prefix: 'Clip end' });
    const revised = await message('0.7', { prefix: 'Clip summary' });
    const go = button(revised.reply, 'go');
    assert.notEqual(go.data, oldGo.data);
    for (const invalid of [
      { actor: 99 }, { isBot: true }, { inaccessible: true },
      { messagePatch: { message_thread_id: 9 } },
      { messagePatch: { chat: { id: -100, type: 'group', title: 'Other chat' } } },
      { messagePatch: { chat: { id: 99, type: 'private', first_name: 'Other user' } } },
      { messagePatch: { from: { id: 999, is_bot: true, first_name: 'Other bot' } } },
    ]) {
      const denied = await callback(go, invalid);
      assert(denied.answer.data.text, 'foreign callback must be acknowledged with denial');
    }
    assert.equal(await count(), initialCount);
    await fixtureSql("UPDATE users SET ban_until = unixepoch('now') * 1000000 + 600000000 WHERE id = 42");
    const banned = await callback(go);
    assert(banned.answer.data.text);
    assert.equal(await count(), initialCount, 'blocked actor cannot confirm a draft');
    await fixtureSql('UPDATE users SET ban_until = NULL WHERE id = 42');
    await restartApp();
    const resumed = await message('/cut', { prefix: 'Clip summary' });
    assert.equal(button(resumed.reply, 'go').data, go.data, 'restart must preserve confirmation identity');

    const uploads = botApi.uploadCalls;
    const sendAsFile = (await fixtureSql('SELECT COALESCE((SELECT send_as_file FROM dl_user_settings WHERE user_id = 42), 0)')) === '1';
    const method = sendAsFile ? 'senddocument' : 'sendvideo';
    botApi.setExpectedUpload({ method, filename: 'media.mp4', mimeType: 'video/mp4', preserveMdat: false });
    const admitted = await confirm(go);
    await h.succeeded(admitted.operationId);
    assert.equal(botApi.uploadCalls, uploads + 1, 'one confirmation must upload exactly once');
    await h.inspectUpload('interactive cut', { video: true, audio: true, durationMin: 0.35, durationMax: 0.8 });
    const frozen = await h.request(admitted.operationId);
    assert.deepEqual(frozen.clip_range, { start_seconds: 0.2, end_seconds: 0.7 });
    const afterAdmission = await count();
    await restartApp();
    const replay = await confirm(go);
    assert.equal(replay.operationId, admitted.operationId);
    assert.equal(await count(), afterAdmission);
    assert.equal(botApi.uploadCalls, uploads + 1, 'repeated confirmation after restart cannot redeliver');
    assert.deepEqual(await h.request(admitted.operationId), frozen);
    botApi.setExpectedUpload(null);

    // Inject the durable boundary reached if the process dies after admission but before
    // recording its operation ID in the conversation. The operation itself is untouched.
    await fixtureSql("UPDATE runtime_cut_sessions SET stage = 'confirming', operation_id = NULL WHERE actor_id = 42");
    await restartApp();
    const recovered = await confirm(go);
    assert.equal(recovered.operationId, admitted.operationId);
    assert.equal(await count(), afterAdmission, 'uncertain admission recovery must reuse the existing job');
    assert.equal(botApi.uploadCalls, uploads + 1);
    assert.equal(await fixtureSql('SELECT stage FROM runtime_cut_sessions WHERE actor_id = 42'), 'submitted');

    await message(`/cut ${url} 0.2`, { prefix: 'Clip end' });
    const duration = await message('0.2', { prefix: 'Clip summary' });
    await callback(button(duration.reply, 'cancel'));
    await message('/cut');
    await callback(button(duration.reply, 'go'));
    assert.equal(await count(), afterAdmission, 'cancelled draft cannot be confirmed');
    await message(`/cut ${url}`);
    await message('/cancel', { prefix: 'Clip cancelled' });
    const russian = await withRussianPreference(fixtureSql, async () => {
      await message(`/cut ${url}`, { language: 'en' });
      return message('0.2', { language: 'en' });
    });
    assert(/[А-Яа-яЁё]/u.test(russian.reply.data.text), 'Russian input must receive localized prompts');
    await withRussianPreference(fixtureSql, () => message('отмена', { language: 'en' }));
    // Settings has its own callback namespace, which must remain reachable.
    const settings = await message('/settings');
    assert(buttons(settings.reply).some(item => item.callback_data?.startsWith('prefs:')));
    await assertWorkspaceClean();
  } finally {
    botApi.setExpectedUpload(null);
    await fixtureSql('UPDATE users SET ban_until = NULL WHERE id = 42');
  }
  return 'interactive cut: persisted input/summary, invalid times, duplicate input, stale buttons, edit/cancel, authorization, localized prompts, actual clipped output and restart-safe confirmation replay';
}
