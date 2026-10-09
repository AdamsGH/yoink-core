import assert from 'node:assert/strict';
import test from 'node:test';
import { setTimeout as delay } from 'node:timers/promises';
import { startUserSessionFixture, runUserSessionScenarios } from './user-session-scenarios.mjs';

async function setup(t) {
  let token;
  const secrets = [];
  const fixture = await startUserSessionFixture(value => { token = value; }, value => secrets.push(value));
  t.after(() => fixture.close());
  const call = async (method, params = {}, override = token) => {
    const response = await fetch(`${fixture.origin}/user${override}/${method}`, {
      method: 'POST', body: new URLSearchParams(params), signal: AbortSignal.timeout(3000),
    });
    const text = await response.text();
    return { status: response.status, body: text ? JSON.parse(text) : null };
  };
  return { fixture, call, secrets, get token() { return token; } };
}

test('user-session fixture matches public proxy result shapes and member form paging', async t => {
  const { fixture, call } = await setup(t);
  const methods = ['getForumTopics', 'getForumTopic', 'getForumTopicLink', 'searchChatMessages', 'getChatHistory', 'getMessageViewers', 'getMessageLink', 'getMessageReadDate', 'getMessageThread', 'getChatMessageByDate'];
  for (const method of methods) {
    const response = await call(method, { chat_id: '-8000000000011', message_thread_id: '77', message_id: '501', date: '1700000000' });
    assert.equal(response.status, 200);
    assert.equal(response.body.ok, true);
    assert.equal(Array.isArray(response.body.result), method === 'getMessageViewers');
  }
  const params = { chat_id: '-8000000000011', filter: 'members', limit: '200', offset: '0' };
  const first = await call('getChatMembers', params);
  const last = await call('getChatMembers', { ...params, offset: '200' });
  assert.equal(first.body.result.length, 200);
  assert.deepEqual(first.body.result.slice(0, 2).map(member => member.user.id), [42, 43]);
  assert.equal(last.body.result.length, 1);
  assert.equal(last.body.result[0].user.id, 880200);
  assert.equal(last.body.result[0].user.firstName, 'Session');
  assert.equal((await call('getChatMembers', { ...params, limit: '201' })).status, 400);
  assert.equal((await call('getChatMembers', { ...params, offset: '-1' })).status, 400);
  assert.equal(fixture.pendingRequests, 0);
});

test('user-session fixture preserves protected rotation, bounded retained calls and monotonic count', async t => {
  const state = await setup(t);
  const old = state.token;
  assert.equal(state.secrets.length, 1);
  await state.fixture.rotate();
  assert.equal(state.secrets.length, 2);
  assert.notEqual(state.token, old);
  assert.equal((await state.call('getForumTopics', {}, old)).status, 401);
  assert.equal(state.fixture.callCount, 0);
  for (let i = 0; i < 70; i++) await state.call('getForumTopics');
  assert.equal(state.fixture.callCount, 70);
  assert.equal(state.fixture.calls.length, 64);
  assert(!JSON.stringify(state.fixture.calls).includes(state.token));
  await state.fixture.setAvailable(false);
  assert.equal(state.token, '');
  await state.fixture.setAvailable(true);
  assert.equal(state.token, state.secrets[1]);
  state.fixture.assertHealthy();
});

test('user-session fixture supplies bounded scan cursors and explicit malformed/over-limit failures', async t => {
  const { fixture, call } = await setup(t);
  fixture.setMode('paged');
  const first = await call('getForumTopics', { offset_forum_topic_id: '0' });
  assert.equal(first.body.result.topics.length, 100);
  assert.equal(first.body.result.next_offset_forum_topic_id, 100);
  assert.equal((await call('getForumTopics', { offset_forum_topic_id: '100' })).body.result.topics.length, 1);
  fixture.setMode('cursor-repeat');
  const repeated = (await call('getForumTopics', { offset_forum_topic_id: '100' })).body.result;
  assert.equal(repeated.next_offset_forum_topic_id, first.body.result.next_offset_forum_topic_id);
  assert.equal(repeated.topics[0].message_thread_id, 101, 'nonprogress cursor has unique subjects, not duplicate-subject rejection');
  fixture.setMode('topics-over-limit');
  assert.equal((await call('getForumTopics')).body.result.topics.length, 101);
  fixture.setMode('members-over-limit');
  assert.equal((await call('getChatMembers', { filter: 'members', limit: '200', offset: '0' })).body.result.length, 201);
  fixture.setMode('malformed');
  assert.equal((await call('getForumTopics')).body.result, null);
  fixture.setMode('error');
  assert.equal((await call('getForumTopics')).status, 502);
  fixture.setMode('ok');
  assert.equal((await call('inventedMethod')).status, 400);
  assert.equal((await call('getForumTopics', { query: 'x'.repeat(8193) })).status, 413);
  assert.throws(() => fixture.setMode('invented-mode'));
});

test('user-session fixture holds captured roster output and joins released handlers', async t => {
  const { fixture, call } = await setup(t);
  const gate = fixture.holdNextMembers();
  const pending = call('getChatMembers', { filter: 'members', limit: '200', offset: '0' });
  try {
    for (let i = 0; !gate.entered && i < 100; i++) await delay(5);
    assert(gate.entered);
    assert.equal(fixture.pendingRequests, 1);
    fixture.setMode('malformed');
  } finally { gate.release(); }
  assert.equal((await pending).body.result.length, 200, 'held response is a stale captured provider snapshot');
  assert.equal(fixture.pendingRequests, 0);
  fixture.assertHealthy();
});

test('user-session runner rejects missing coordinator context before any external interaction', async () => {
  await assert.rejects(runUserSessionScenarios({}), /coordinator must supply waitFor/);
  await assert.rejects(runUserSessionScenarios({ waitFor() {} }), /coordinator must supply context.botApi/);
});
