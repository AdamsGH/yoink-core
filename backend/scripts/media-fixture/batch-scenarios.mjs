import assert from 'node:assert/strict';
import { commandHarness } from './command-harness.mjs';
import { assertPrivateCaption, assertSuccessCleanup } from './presentation.mjs';

export async function runBatchScenarios(context) {
  const { botApi, sourcePort, sourceFixture, fixtureSql, queryJob, waitFor, assertWorkspaceClean, playlistMedia, imageBytes, restartApp } = context;
  const h = commandHarness(context, 1300);
  const origin = `http://127.0.0.1:${sourcePort}`;
  const album = indices => indices.map(index => ({ kind: 'video', filename: 'media.mp4', mimeType: 'video/mp4', bytes: playlistMedia[(index - 1) % playlistMedia.length], preserveMdat: true }));
  const terminal = async (id, state) => waitFor(async () => (await queryJob(id)).startsWith(`${state}|`), `batch ${state}`, 45000);
  const counts = id => fixtureSql(`SELECT planned_artifact_count || '|' || delivered_artifact_count FROM runtime_operations WHERE id='${id}'`);
  const journal = id => fixtureSql(`SELECT ordinal || '|' || artifact_count || '|' || status || '|' || COALESCE(json_array_length(message_ids),0) FROM runtime_delivery_units WHERE operation_id='${id}' ORDER BY ordinal`);
  try {
    botApi.setExpectedUpload({ method: 'sendphoto', preserveMdat: false, filename: 'image.png', mimeType: 'image/png' });
    const photo = await h.submit(`/image ${origin}/image.png`);
    await h.succeeded(photo.operationId);
    assert.deepEqual(botApi.lastUploadedBytes, imageBytes, 'direct gallery image must arrive intact');
    assert.equal(await counts(photo.operationId), '1|1');
    assert.equal(await journal(photo.operationId), '0|1|sent|1');
    await h.replay(`/image ${origin}/image.png`, photo);
    botApi.setExpectedUpload(null);

    const command = `/playlist 1-2 ${origin}/playlist.html?count=2`;
    botApi.setExpectedAlbum(album([1, 2]));
    const before = botApi.uploadCalls;
    const pair = await h.submit(command);
    await h.succeeded(pair.operationId);
    assert.equal(botApi.uploadCalls, before + 1, 'two videos require one album API call');
    assert.equal(botApi.lastGroupBytes.length, 2);
    assert.equal(await counts(pair.operationId), '2|2');
    assert.equal(await journal(pair.operationId), '0|2|sent|2');
    const albumSend = botApi.calls.slice(pair.callStart).find(call => call.method === 'sendmediagroup');
    assertPrivateCaption({ caption: albumSend.captions[0], parse_mode: albumSend.parse_modes[0], caption_above: albumSend.caption_above[0] }, `${origin}/playlist.html?count=2`);
    assert.equal(albumSend.captions[1], undefined, 'album caption must appear only on the first item');
    await assertSuccessCleanup(context, pair);
    await h.replay(command, pair);
    const sourceBefore = sourceFixture.mediaRequests;
    const cachedBefore = botApi.cachedCalls;
    const cached = await h.submit(command);
    await h.succeeded(cached.operationId);
    assert.equal(botApi.cachedCalls, cachedBefore + 1, 'batch cache reuses the entire confirmed album');
    assert.equal(botApi.uploadCalls, before + 1);
    assert.equal(sourceFixture.mediaRequests, sourceBefore);
    const cachedAlbum = botApi.calls.slice(cached.callStart).find(call => call.method === 'sendmediagroup');
    assert.deepEqual(cachedAlbum.captions, albumSend.captions, 'album cache must preserve its first-item caption');
    await assertSuccessCleanup(context, cached);

    botApi.setSendMode('invalid-file-id-once');
    const recoverySources = sourceFixture.mediaRequests;
    const recovered = await h.submit(command);
    await h.succeeded(recovered.operationId);
    assert.equal(botApi.cachedCalls, cachedBefore + 2, 'invalid album ID is attempted once');
    assert.equal(botApi.uploadCalls, before + 2, 'invalid album ID triggers one fresh album upload');
    assert(sourceFixture.mediaRequests > recoverySources);
    assert.equal(await counts(recovered.operationId), '2|2');

    botApi.setExpectedAlbum(album([2, 1]));
    const reverse = await h.submit(`/playlist 2-1 ${origin}/playlist.html?count=2&case=reverse`);
    await h.succeeded(reverse.operationId);
    assert.equal(await counts(reverse.operationId), '2|2');

    botApi.setExpectedAlbum(album([1, 2]));
    botApi.setSendMode('short-response');
    const unknownCommand = `/playlist 1-2 ${origin}/playlist.html?count=2&case=short-response`;
    const unknown = await h.submit(unknownCommand);
    await terminal(unknown.operationId, 'delivery_unknown');
    assert.equal(await counts(unknown.operationId), '2|0');
    assert.equal(await journal(unknown.operationId), '0|2|unknown|0');
    botApi.setSendMode('success');
    await h.replay(unknownCommand, unknown);

    botApi.setExpectedAlbum(album(Array.from({ length: 10 }, (_, i) => i + 1)));
    botApi.setSendPlan(['success', 'api-error']);
    const partialCommand = `/playlist 1-11 ${origin}/playlist.html?count=11&case=partial`;
    const partial = await h.submit(partialCommand);
    await terminal(partial.operationId, 'delivery_partial');
    assert.equal(await counts(partial.operationId), '11|10');
    assert.equal(await journal(partial.operationId), '0|10|sent|10\n1|1|rejected|0');
    botApi.setSendMode('success');
    await assertWorkspaceClean();
    await restartApp();
    await h.replay(partialCommand, partial);
    await h.replay(unknownCommand, unknown);
    assert.equal(await counts(partial.operationId), '11|10', 'restart must preserve the confirmed prefix');

    const sendsBefore = botApi.sendCalls;
    const broken = await h.submit(`/playlist 1-2 ${origin}/playlist.html?count=2&broken=2`);
    await terminal(broken.operationId, 'failed');
    assert.equal(botApi.sendCalls, sendsBefore, 'a failed source batch must not send its downloaded prefix');
    assert.equal(await journal(broken.operationId), '');
    for (const retained of [unknown, partial, broken]) {
      await waitFor(async () => await fixtureSql(`SELECT finished FROM runtime_progress_notices WHERE operation_id='${retained.operationId}'`) === '1', 'terminal failure notice');
      const noticeId = Number(await fixtureSql(`SELECT message_id FROM runtime_progress_notices WHERE operation_id='${retained.operationId}'`));
      assert(!botApi.calls.some(call => call.method === 'deletemessage' && call.data.chat_id === 42
        && [retained.messageId, noticeId].includes(call.data.message_id)), 'failed, partial and uncertain deliveries must retain their messages');
    }
    await assertWorkspaceClean();
  } finally {
    botApi.setSendMode('success');
    botApi.setSendPlan([]);
    botApi.setExpectedUpload(null);
    botApi.setExpectedAlbum(null);
  }
  return 'real gallery photo, playlist albums/order/cache, uncertain receipts and durable partial-prefix delivery passed';
}
