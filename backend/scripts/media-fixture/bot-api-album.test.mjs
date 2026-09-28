import test from 'node:test';
import assert from 'node:assert/strict';
import { BotApiFixture } from './bot-api.mjs';

const bytes = [Buffer.alloc(160, 1), Buffer.alloc(180, 2)];
const contract = bytes.map(value => ({ kind: 'photo', filename: 'image.png', mimeType: 'image/png', bytes: value }));
function uploadForm({ reverse = false, missing = false, extra = false } = {}) {
  const form = new FormData();
  form.set('chat_id', '42');
  form.set('media', JSON.stringify(bytes.map((_, i) => ({ type: 'photo', media: `attach://image${i}` }))));
  for (let i = 0; i < (missing ? 1 : 2); i++) form.set(`image${i}`, new Blob([bytes[reverse ? 1 - i : i]], { type: 'image/png' }), 'image.png');
  if (extra) form.set('unused', new Blob([bytes[0]]), 'unused.png');
  return form;
}
async function start(t) {
  const api = new BotApiFixture({ mediaBytes: bytes[0] });
  api.setExpectedAlbum(contract);
  const port = await api.start();
  t.after(() => api.close());
  const url = `http://127.0.0.1:${port}/bottoken/sendMediaGroup`;
  return { api, url };
}

test('album fixture verifies ordered attachments and accepts only confirmed cached IDs', async t => {
  const { api, url } = await start(t);
  const response = await fetch(url, { method: 'POST', body: uploadForm() });
  const uploaded = await response.json();
  assert.equal(uploaded.result.length, 2);
  assert.notEqual(uploaded.result[0].message_id, uploaded.result[1].message_id);
  assert.deepEqual(api.lastGroupBytes, bytes);
  assert.equal(api.uploadCalls, 1);
  const cached = await fetch(url, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ chat_id: 42, media: uploaded.result.map(message => ({ type: 'photo', media: message.photo[0].file_id })) }) });
  assert.equal((await cached.json()).result.length, 2);
  assert.equal(api.cachedCalls, 1);
  assert.equal(api.error, undefined);
});

test('album fixture rejects reordered, missing and unused uploads', async t => {
  for (const [name, options, error] of [['order', { reverse: true }, /content\/order/], ['missing', { missing: true }, /reference must resolve/], ['unused', { extra: true }, /unreferenced files/]]) {
    await t.test(name, async t => {
      const { api, url } = await start(t);
      await assert.rejects(fetch(url, { method: 'POST', body: uploadForm(options) }));
      assert.match(api.error.message, error);
    });
  }
});

test('album fixture can return uncertain receipts and bounded one-shot failures', async t => {
  const { api, url } = await start(t);
  api.setSendPlan(['short-response', 'duplicate-response', 'rate-limit-once']);
  const post = async () => (await fetch(url, { method: 'POST', body: uploadForm() })).json();
  assert.equal((await post()).result.length, 1);
  const duplicated = await post();
  assert.equal(duplicated.result[0].message_id, duplicated.result[1].message_id);
  assert.equal((await post()).error_code, 429);
  assert.equal((await post()).result.length, 2);
});
