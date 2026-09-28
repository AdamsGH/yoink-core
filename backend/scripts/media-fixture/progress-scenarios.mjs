import assert from 'node:assert/strict';
import { setTimeout as sleep } from 'node:timers/promises';
import { commandHarness } from './command-harness.mjs';

export async function runProgressScenarios(context) {
  const { botApi, sourceFixture, sourcePort, fixtureSql, waitFor, restartApp } = context;
  const h = commandHarness(context, 1800);
  const edits = id => botApi.calls.filter(call => call.method === 'editmessagetext' && call.data.text.includes(id));
  const replies = () => botApi.calls.filter(call => call.method === 'sendmessage').length;
  const checkpoint = id => fixtureSql(`SELECT finished || '|' || COALESCE(last_text,'') FROM runtime_progress_notices WHERE operation_id='${id}'`);
  async function finished(id) { await waitFor(async () => (await checkpoint(id)).startsWith('1|'), `finished progress ${id}`); }
  await waitFor(async () => await fixtureSql('SELECT count(*) FROM runtime_progress_notices WHERE finished=0') === '0', 'drained earlier progress notices', 90000);
  botApi.setExpectedUpload(null);
  botApi.setSendMode('success');
  sourceFixture.closeProgressGate();
  botApi.setEditMode('rate-limit-once');
  const first = await h.submit(`/video http://127.0.0.1:${sourcePort}/progress.mp4?case=progress`);
  const id = first.operationId;
  try {
    await waitFor(async () => /Downloading\. \d+%/.test(await checkpoint(id)), 'real worker percentage acknowledged by Telegram');
    const rateLimited = edits(id).find(call => call.status === 'rate-limit-once');
    assert(rateLimited, 'progress must exercise Telegram retry_after');
    const retried = edits(id).find(call => call.receivedAt > rateLimited.receivedAt);
    assert(retried && retried.receivedAt - rateLimited.receivedAt >= 1900, 'progress must honor the two-second retry_after');
    await sleep(1200);
    const unchangedEdits = edits(id).length;
    await sleep(2200);
    assert.equal(edits(id).length, unchangedEdits, 'unchanged progress must not issue edits');
    botApi.setEditMode('socket-drop-once', ': succeeded ');
    sourceFixture.releaseProgressResponses();
    await h.succeeded(id);
    await waitFor(() => edits(id).some(call => call.status === 'socket-drop-once'), 'lost terminal edit acknowledgement');
    assert((await checkpoint(id)).startsWith('0|'), 'lost edit acknowledgement must remain pending');
    const sends = botApi.sendCalls;
    const notices = replies();
    await restartApp();
    await finished(id);
    assert.equal(botApi.sendCalls, sends, 'progress recovery must not resend media');
    assert.equal(replies(), notices, 'progress recovery must not create another message');
    const attempts = edits(id);
    assert.equal(new Set(attempts.map(call => call.data.message_id)).size, 1, 'restart edits only the original confirmed message');
    const dropped = attempts.find(call => call.status === 'socket-drop-once');
    assert(attempts.some(call => call.receivedAt > dropped.receivedAt && call.data.text === dropped.data.text), 'restart retries identical terminal edit, accepting message-not-modified');
    assert.match(await checkpoint(id), /^1\|.*succeeded.*Delivered files: 1\/1\./);
    const terminalCount = edits(id).length;
    await sleep(2200);
    assert.equal(edits(id).length, terminalCount, 'terminal acknowledgement stops progress');
  } finally { sourceFixture.releaseProgressResponses(); botApi.setEditMode('success'); }

  botApi.setEditMode('deleted');
  const deletedBefore = replies();
  const deleted = await h.submit(`/video http://127.0.0.1:${sourcePort}/media?case=progress-deleted`);
  await h.succeeded(deleted.operationId);
  await finished(deleted.operationId);
  const deletedCount = edits(deleted.operationId).length;
  await sleep(2200);
  assert.equal(edits(deleted.operationId).length, deletedCount);
  assert.equal(deletedCount, 1, 'deleted status message stops permanently after one failed edit');
  assert.equal(replies(), deletedBefore + 1, 'deleted status message is not recreated');
  botApi.setEditMode('success');

  const russian = await h.submit(`/video http://127.0.0.1:${sourcePort}/media?case=progress-russian`, { language: 'ru' });
  await h.succeeded(russian.operationId);
  await finished(russian.operationId);
  assert.match(await checkpoint(russian.operationId), /^1\|Операция .*завершено.*Доставлено файлов: 1\/1\./);

  const role = await fixtureSql('SELECT role FROM users WHERE id=42');
  assert(['owner', 'admin', 'moderator', 'user'].includes(role));
  sourceFixture.closeProgressGate();
  const revoked = await h.submit(`/video http://127.0.0.1:${sourcePort}/progress.mp4?case=progress-revoked`);
  try {
    await waitFor(() => edits(revoked.operationId).some(call => /Downloading\. \d+%/.test(call.data.text)), 'progress before revocation');
    await fixtureSql("UPDATE users SET role='banned' WHERE id=42");
    await finished(revoked.operationId);
    const afterRevocation = edits(revoked.operationId).length;
    sourceFixture.releaseProgressResponses();
    await waitFor(async () => (await context.queryJob(revoked.operationId)).startsWith('failed|'), 'revoked operation delivery denied');
    await sleep(2200);
    assert.equal(edits(revoked.operationId).length, afterRevocation, 'revocation stops progress edits');
  } finally {
    sourceFixture.releaseProgressResponses();
    await fixtureSql(`UPDATE users SET role='${role}' WHERE id=42`);
  }
  await context.assertWorkspaceClean();
  return 'Progress passed real worker percentages, unchanged-text coalescing, retry_after, lost acknowledgement/restart, deleted message, EN/RU and revocation checks';
}
