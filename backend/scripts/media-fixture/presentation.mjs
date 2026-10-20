import assert from 'node:assert/strict';
import { setTimeout as sleep } from 'node:timers/promises';
import { commandHarness } from './command-harness.mjs';

export function assertPrivateCaption(call, url, { above = true, range } = {}) {
  const source = new URL(url).href.replaceAll('&', '&amp;').replaceAll('"', '&quot;');
  const prefix = `<a href="${source}"><b>`;
  assert.equal(call.parse_mode, 'HTML', 'media caption must carry explicit HTML formatting');
  assert(call.caption?.startsWith(prefix), 'title must link to its original media source');
  const end = call.caption.indexOf('</b></a>');
  assert(end > prefix.length, 'linked title must not be empty');
  const title = call.caption.slice(prefix.length, end);
  assert(!/[<>]/.test(title), 'extractor title must not inject HTML');
  assert(!['source', 'источник'].includes(title), 'known extractor title must reach fresh and cached captions');
  assert.equal(call.caption_above, above, 'private visual captions must appear above media');
  if (range) assert(call.caption.endsWith(range), 'cut caption must include its requested range');
}

export async function assertSuccessCleanup(context, submission) {
  const { botApi, fixtureSql, waitFor } = context;
  const notice = await waitFor(async () => {
    const row = await fixtureSql(`SELECT message_id, finished FROM runtime_progress_notices WHERE operation_id='${submission.operationId}'`);
    return /^\d+\|1$/.test(row) ? Number(row.split('|')[0]) : false;
  }, 'successful delivery message cleanup', 20000);
  for (const id of [submission.messageId, notice]) {
    assert(botApi.calls.slice(submission.callStart).some(call => call.method === 'deletemessage'
      && call.data.chat_id === 42 && call.data.message_id === id && ['success', 'not-found'].includes(call.status)),
    'success must remove both private input and progress notice');
  }
  assert(!botApi.calls.slice(submission.callStart).some(call => call.method === 'editmessagetext'
    && /^(Done\.|Готово\.)/.test(call.data.text)), 'success must not leave a terminal status message');
}

export async function runPresentationScenarios(context) {
  const { botApi, sourceFixture, sourcePort, fixtureSql, waitFor, assertWorkspaceClean } = context;
  const h = commandHarness(context, 51000);
  const url = `http://127.0.0.1:${sourcePort}/media?case=presentation`;
  const media = () => botApi.calls.filter(call => call.method === 'sendvideo').at(-1);
  const actions = from => botApi.calls.slice(from).filter(call => call.method === 'sendchataction' && call.data.action === 'upload_video');
  botApi.setExpectedUpload({ method: 'sendvideo', filename: 'media.mp4', mimeType: 'video/mp4', preserveMdat: true });
  const release = botApi.holdNextUpload({ success: true });
  let first;
  try {
    try {
      first = await h.submit(`/video ${url}`);
      await waitFor(() => media()?.status === 'hold-success', 'held successful media upload');
      await waitFor(() => actions(first.callStart).length >= 2, 'native uploading status refresh', 12000);
      const status = actions(first.callStart);
      assert(status[1].receivedAt - status[0].receivedAt >= 3900, 'upload status must refresh without flooding');
      assert(!botApi.calls.slice(first.callStart).some(call => call.method === 'deletemessage'), 'messages must remain until confirmed delivery');
      assertPrivateCaption(media(), url);
      botApi.setDeleteMode('rate-limit-once', first.messageId);
    } finally { release(); }
    await h.succeeded(first.operationId);
    await assertSuccessCleanup(context, first);
    const deleted = botApi.calls.filter(call => call.method === 'deletemessage' && call.data.message_id === first.messageId);
    assert.equal(deleted[0].status, 'rate-limit-once');
    assert(deleted[1].receivedAt - deleted[0].receivedAt >= 1900, 'cleanup must honor Telegram retry_after');
    const firstCaption = media().caption;
    const actionCount = actions(first.callStart).length;
    await sleep(4200);
    assert.equal(actions(first.callStart).length, actionCount, 'upload actions must stop with delivery');

    await waitFor(async () => Number(await fixtureSql(`SELECT count(*) FROM runtime_media_cache WHERE cache_key=(SELECT cache_key FROM runtime_operations WHERE id='${first.operationId}')`)) > 0, 'caption metadata cache publication');
    const sourceRequests = sourceFixture.mediaRequests;
    const cachedCount = botApi.cachedCalls;
    const cached = await h.submit(`/video ${url}`);
    await h.succeeded(cached.operationId);
    await assertSuccessCleanup(context, cached);
    assert.equal(botApi.cachedCalls, cachedCount + 1);
    assert.equal(sourceFixture.mediaRequests, sourceRequests, 'cache reuse must not refetch title metadata');
    assertPrivateCaption(media(), url);
    assert.equal(media().caption, firstCaption, 'cache must preserve linked-title presentation');
    const delivered = media();
    const replyStart = botApi.calls.length;
    botApi.enqueue(42, 42, '/job', 51999, 'en', { replyToMessage: delivered.messageId });
    await waitFor(() => botApi.calls.slice(replyStart).some(call => call.method === 'sendmessage' && call.data.text === 'Done. Delivered files: 1/1.'), 'explicit job reply to retained media after input cleanup');

    for (const [command, method, filename, mimeType] of [
      ['audio', 'sendaudio', 'audio.m4a', 'audio/mp4'],
      ['video --send-as-file true', 'senddocument', 'media.mp4', 'video/mp4'],
    ]) {
      const source = `${url}&kind=${method}`;
      botApi.setExpectedUpload({ method, filename, mimeType, preserveMdat: false });
      const submitted = await h.submit(`/${command} ${source}`);
      await h.succeeded(submitted.operationId);
      await assertSuccessCleanup(context, submitted);
      const sent = botApi.calls.slice(submitted.callStart).find(call => call.method === method);
      assertPrivateCaption(sent, source, { above: false });
      const calls = botApi.calls.slice(submitted.callStart);
      const actionIndex = calls.findIndex(call => call.method === 'sendchataction'
        && call.data.action === (method === 'sendaudio' ? 'upload_voice' : 'upload_document'));
      assert(actionIndex >= 0 && actionIndex < calls.indexOf(sent), 'even fast sends must announce upload activity before the media');
    }
    botApi.setExpectedUpload({ method: 'sendvideo', filename: 'media.mp4', mimeType: 'video/mp4', preserveMdat: true });
    const finishCancelledUpload = botApi.holdNextUpload({ success: true });
    let cancelled;
    try {
      cancelled = await h.submit(`/video ${url}&cancel=1`);
      await waitFor(() => actions(cancelled.callStart).length >= 2, 'upload activity before cancellation', 12000);
      const noticeId = await waitFor(async () => Number(await fixtureSql(`SELECT message_id FROM runtime_progress_notices WHERE operation_id='${cancelled.operationId}'`)) || false, 'cancellable progress notice');
      const cancelStart = botApi.calls.length;
      botApi.enqueue(42, 42, '/cancel', 51998, 'en', { replyToMessage: noticeId });
      await waitFor(() => botApi.calls.slice(cancelStart).some(call => call.method === 'sendmessage' && call.data.text === 'Cancellation requested.'), 'in-flight cancellation acknowledgement');
      const beforeCancel = actions(cancelled.callStart).length;
      await sleep(4200);
      assert.equal(actions(cancelled.callStart).length, beforeCancel, 'cancellation must stop upload actions while the receipt remains pending');
      assert(!botApi.calls.slice(cancelled.callStart).some(call => call.method === 'deletemessage'), 'cancellation must not delete messages before confirmed delivery');
    } finally { finishCancelledUpload(); }
    await h.succeeded(cancelled.operationId);
    await assertSuccessCleanup(context, cancelled);
    const lostAckUrl = `${url}&lost-queue-ack=1`;
    for (let attempt = 0; attempt < 2; attempt++) {
      botApi.setTextMode('socket-drop-once');
      const submitted = await h.submit(`/video ${lostAckUrl}`);
      await h.succeeded(submitted.operationId);
      await waitFor(async () => (await fixtureSql(`SELECT message_id, finished FROM runtime_progress_notices WHERE operation_id='${submitted.operationId}'`)) === `${submitted.messageId}|1`, 'private cleanup without a confirmed progress ID');
      const calls = botApi.calls.slice(submitted.callStart);
      assert(calls.some(call => call.method === 'deletemessage' && call.data.message_id === submitted.messageId && call.status === 'success'));
      assert(!calls.some(call => call.method === 'editmessagetext' && Number(call.data.message_id) === submitted.messageId), 'input references must never become editable bot notices');
      assert(!calls.some(call => call.method === 'deletemessage' && call.data.message_id !== submitted.messageId), 'an unacknowledged bot message ID must not be guessed');
      assertPrivateCaption(media(), lostAckUrl);
      if (attempt === 0) await waitFor(async () => Number(await fixtureSql(`SELECT count(*) FROM runtime_media_cache WHERE cache_key=(SELECT cache_key FROM runtime_operations WHERE id='${submitted.operationId}')`)) > 0, 'cache publication before lost-ack replay');
    }
    await assertWorkspaceClean();
    assert.equal(botApi.error, undefined);
    return 'presentation passed linked-title captions, cache parity, native upload activity completion/cancellation, success-only cleanup/retry_after and reply-to-media job lookup';
  } finally {
    botApi.setSendMode('success');
    botApi.setDeleteMode('success');
    botApi.setExpectedUpload(null);
  }
}
