import assert from 'node:assert/strict';
import { setTimeout as sleep } from 'node:timers/promises';
import { commandHarness } from './command-harness.mjs';

export async function runGroupScenarios(context) {
  const { botApi, sourceFixture, sourcePort, fixtureSql, queryJob, waitFor, restartGroups, playlistMedia } = context;
  const h = commandHarness(context, 2200);
  const user = 8000000000011, restricted = 8000000000012, group = -8000000000011;
  const origin = `http://127.0.0.1:${sourcePort}`;
  const route = { actor: user, chat: group, thread: 18 };
  const sends = () => botApi.calls.filter(call => call.method === 'sendmessage');
  const edits = id => botApi.calls.filter(call => call.method === 'editmessagetext' && call.data.text.includes(id));
  const finished = id => waitFor(async () => await fixtureSql(`SELECT finished FROM runtime_progress_notices WHERE operation_id='${id}'`) === '1', `finished group progress ${id}`);
  const terminal = (id, state) => waitFor(async () => (await queryJob(id)).startsWith(`${state}|`), `group ${state}`, 30000);
  let nextId = 2400;
  async function silent(cases) {
    const count = await fixtureSql('SELECT count(*) FROM runtime_operations');
    const before = sends().length;
    for (const [actor, chat, thread, extra = {}] of cases) botApi.enqueue(actor, chat, extra.command ?? `/video ${origin}/media?case=denied-group`, nextId++, 'en', { thread, ...extra });
    // Positive control proves that the polling process is alive and consuming updates.
    botApi.enqueue(42, 42, '/help', nextId++);
    await waitFor(() => sends().length > before, 'private polling control');
    await sleep(800);
    assert.equal(sends().length, before + 1, 'unauthorized group commands remain silent');
    assert.equal(await fixtureSql('SELECT count(*) FROM runtime_operations'), count, 'unauthorized group commands create no jobs');
  }
  await waitFor(async () => await fixtureSql('SELECT count(*) FROM runtime_progress_notices WHERE finished=0') === '0', 'drain earlier progress', 90000);
  await silent([[user, group, 18], [42, group, 18]]);
  await restartGroups(true);
  try {
    await silent([
      [user, group, 17], [user, group, 19], [user, -8000000000012], [user, -12345],
      [8000000000013, group, 18], [8000000000014, group, 18], [777777, group, 18],
      [user, group, 18, { isBot: true }],
      [42, group, 18, { senderChat: { id: group, type: 'supergroup', title: 'Anonymous sender' } }],
      [user, user], [user, group, 18, { command: '/status' }],
    ]);
    const bareSource = `${origin}/media?case=group-bare-url`;
    const bareMetadata = { entities: [{ type: 'url', offset: 0, length: bareSource.length }], command: bareSource };
    await silent([
      [user, group, 18, bareMetadata],
      [user, group, 18, { ...bareMetadata, replyToBot: 999 }],
      [user, group, 18, { ...bareMetadata, viaBot: 999 }],
      [user, group, 17, { ...bareMetadata, replyToBot: 123456 }],
      [777777, group, 18, { ...bareMetadata, viaBot: 123456 }],
    ]);
    for (const trigger of [{ replyToBot: 123456 }, { viaBot: 123456 }]) {
      const sent = await h.submit(bareSource, { ...route, ...bareMetadata, ...trigger });
      await h.succeeded(sent.operationId);
      await finished(sent.operationId);
      const upload = botApi.calls.slice(sent.callStart).find(call => call.method === 'sendvideo');
      assert.equal(upload.chat_id, group);
      assert.equal(upload.message_thread_id, 18);
      assert.equal(upload.reply_to_message_id, sent.messageId);
      await h.replay(bareSource, sent, { ...route, ...bareMetadata, ...trigger });
    }
    const command = `/video ${origin}/media?case=group-route`;
    const first = await h.submit(command, route);
    await h.succeeded(first.operationId);
    await finished(first.operationId);
    const firstSend = botApi.calls.slice(first.callStart).find(call => call.method === 'sendvideo');
    assert.equal(firstSend.chat_id, group);
    assert.equal(firstSend.message_thread_id, 18);
    assert.equal(firstSend.reply_to_message_id, first.messageId);
    const request = await h.request(first.operationId);
    assert.equal(request.actor_id, user);
    assert.equal(request.target.chat_id, group);
    assert.equal(request.target.thread_id, 18);
    const notice = sends().find(call => call.data.text.includes(first.operationId));
    assert.equal(notice.data.message_thread_id, 18);
    assert(edits(first.operationId).every(call => Number(call.data.chat_id) === group && call.data.message_id === notice.messageId));
    const mediaCount = botApi.sendCalls;
    const replay = await h.submit(command, { ...route, messageId: first.messageId });
    assert.equal(replay.operationId, first.operationId);
    assert.equal(botApi.sendCalls, mediaCount);

    const cachedBefore = botApi.cachedCalls;
    const plain = await h.submit(command, { actor: user, chat: group });
    await h.succeeded(plain.operationId);
    assert.equal(botApi.cachedCalls, cachedBefore + 1, 'same media cache can serve a different authorized topic');
    const cached = botApi.calls.slice(plain.callStart).find(call => call.method === 'sendvideo');
    assert.equal(cached.chat_id, group);
    assert.equal(cached.message_thread_id, undefined);
    assert.equal(cached.reply_to_message_id, plain.messageId);

    for (const command of [`/job ${first.operationId}`, `/cancel ${first.operationId}`]) {
      const before = sends().length;
      botApi.enqueue(user, group, command, nextId++);
      await waitFor(() => sends().length > before, 'cross-topic query rejected');
      const reply = sends().at(-1);
      assert(!reply.data.text.includes(first.operationId), 'cross-topic response must not expose or cancel the operation');
    }
    const local = await h.submit(`/video ${origin}/media?case=group-local-grant`, { actor: restricted, chat: group, thread: 18 });
    await h.succeeded(local.operationId);
    assert.equal(await fixtureSql(`SELECT role FROM users WHERE id=${restricted}`), 'restricted', 'group elevation must not mutate global identity');

    botApi.setExpectedAlbum([0, 1].map(index => ({ kind: 'video', filename: 'media.mp4', mimeType: 'video/mp4', bytes: playlistMedia[index], preserveMdat: true })));
    const album = await h.submit(`/playlist 1-2 ${origin}/playlist.html?count=2&case=group-album`, route);
    await h.succeeded(album.operationId);
    const albumSend = botApi.calls.slice(album.callStart).find(call => call.method === 'sendmediagroup');
    assert.equal(albumSend.chat_id, group);
    assert.equal(albumSend.message_thread_id, 18);
    botApi.setExpectedAlbum(null);

    sourceFixture.closeProgressGate();
    const revoked = await h.submit(`/video ${origin}/progress.mp4?case=group-revocation`, route);
    await waitFor(() => edits(revoked.operationId).some(call => /Downloading\. \d+%/.test(call.data.text)), 'group progress before topic revocation');
    const sendsBeforeRevocation = botApi.sendCalls;
    await fixtureSql(`UPDATE thread_policies SET enabled=0 WHERE group_id=${group} AND thread_id=18`);
    await finished(revoked.operationId);
    const editsAfterRevocation = edits(revoked.operationId).length;
    sourceFixture.releaseProgressResponses();
    await terminal(revoked.operationId, 'failed');
    assert.equal(botApi.sendCalls, sendsBeforeRevocation, 'revoked topic cannot receive media');
    assert.equal(edits(revoked.operationId).length, editsAfterRevocation, 'revoked topic stops progress edits');
    await fixtureSql(`UPDATE thread_policies SET enabled=1 WHERE group_id=${group} AND thread_id=18`);

    botApi.setSendMode('wrong-thread');
    const uncertain = await h.submit(`/video ${origin}/media?case=wrong-topic-response`, route);
    await terminal(uncertain.operationId, 'delivery_unknown');
    botApi.setSendMode('success');
    const beforeRestart = botApi.sendCalls;
    await restartGroups(true);
    const repeated = await h.submit(`/video ${origin}/media?case=wrong-topic-response`, { ...route, messageId: uncertain.messageId });
    assert.equal(repeated.operationId, uncertain.operationId);
    assert((await queryJob(uncertain.operationId)).startsWith('delivery_unknown|'));
    assert.equal(botApi.sendCalls, beforeRestart, 'unconfirmed destination must not retry media');
    await context.assertWorkspaceClean();
  } finally {
    sourceFixture.releaseProgressResponses();
    botApi.setSendMode('success');
    botApi.setExpectedAlbum(null);
    await fixtureSql(`UPDATE thread_policies SET enabled=1 WHERE group_id=${group} AND thread_id=18`);
    await restartGroups(false);
  }
  await silent([[user, group, 18]]);
  return 'group opt-in, silent policy denials, local role grants, exact topic/reply routing, cache/album destinations, scoped job/cancel, revocation and uncertain-destination restart passed';
}
