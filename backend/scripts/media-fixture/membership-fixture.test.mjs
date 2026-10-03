import test from 'node:test';
import assert from 'node:assert/strict';
import { BotApiFixture } from './bot-api.mjs';

test('membership fixture preserves raw tags and acknowledges only the next offset', async t => {
  const api = new BotApiFixture({ mediaBytes: Buffer.alloc(0) });
  const port = await api.start();
  t.after(() => api.close());
  const poll = async (offset, allowed_updates) => (await fetch(`http://127.0.0.1:${port}/bottoken/getUpdates`, {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ offset, allowed_updates }),
  })).json();
  const botId = api.enqueueMembership(-10, 123456, 'member', { bot: true });
  const memberId = api.enqueueMembership(-10, 43, 'restricted', { tag: 'Подписчик', present: true });
  const kinds = ['my_chat_member', 'chat_member'];
  const first = (await poll(0, kinds)).result;
  assert.equal(first.length, 2);
  assert.equal(first[0].update_id, botId);
  assert.equal(first[0].my_chat_member.new_chat_member.user.id, 123456);
  assert.equal(first[1].chat_member.new_chat_member.tag, 'Подписчик');
  assert.equal(first[1].chat_member.new_chat_member.is_member, true);
  assert.equal(first[1].chat_member.from.id, 42, 'event actor differs from subject');
  assert.deepEqual((await poll(0, kinds)).result, first, 'no acknowledgment means replay');
  assert.deepEqual((await poll(memberId, kinds)).result, [first[1]], 'offset acknowledges preceding updates only');
  assert.deepEqual((await poll(memberId + 1, kinds)).result, []);
  assert.deepEqual(api.polls.at(-1), { offset: memberId + 1, allowed_updates: kinds });
  assert.equal(api.error, undefined);
});
