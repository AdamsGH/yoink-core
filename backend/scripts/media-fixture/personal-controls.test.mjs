import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';
import { BotApiFixture } from './bot-api.mjs';
import { runPersonalControlsScenarios } from './personal-controls-scenarios.mjs';

test('personal controls acceptance is opt-in and covers credential fences without inline work', async () => {
  assert.equal(typeof runPersonalControlsScenarios, 'function');
  const source = await readFile(new URL('./personal-controls-scenarios.mjs', import.meta.url), 'utf8');
  const runner = await readFile(new URL('../test-media.mjs', import.meta.url), 'utf8');
  assert.match(runner, /if \(scenario === 'personal-controls'\) summaries.push\(await runPersonalControlsScenarios/);
  for (const contract of ['const actor = 43', 'enqueueDocument', 'setCookieImportResponse', 'frozen.network.cookie', 'proxyFixture.authHits', 'proxyFixture.hits', 'cookie:page:0', "role='restricted'", 'ban_until=', 'sender_chat', 'message_thread_id', 'ownerCookies', 'ownerSettings', 'saved.reverse()']) assert(source.includes(contract), contract);
  assert(!source.includes('inline_query'));
});

test('actual document fixture serves a nonowner private upload without logging jar bytes', async t => {
  const api = new BotApiFixture({ mediaBytes: Buffer.alloc(8) });
  const port = await api.start();
  t.after(() => api.close());
  const content = Buffer.from('# Netscape HTTP Cookie File\nexample.test\tFALSE\t/\tFALSE\t4102444800\tsession\topaque-personal-secret\n');
  api.setDocumentFile('personal-file', content);
  api.enqueueDocument(43, 43, '/cookie example.test', 2400000, 'personal-file', { filename: 'example.test.txt' });
  const post = async (method, data) => fetch(`http://127.0.0.1:${port}/token/${method}`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(data) }).then(r => r.json());
  const update = (await post('getUpdates', {})).result[0].message;
  assert.equal(update.from.id, 43);
  assert.equal(update.chat.id, 43);
  assert.equal(update.chat.type, 'private');
  assert.equal(update.from.is_bot, false);
  const file = (await post('getFile', { file_id: update.document.file_id })).result;
  const response = await fetch(`http://127.0.0.1:${port}/file/token/${file.file_path}`);
  assert(Buffer.from(await response.arrayBuffer()).equals(content), 'registered personal document bytes');
  assert(!JSON.stringify(api.calls).includes('opaque-personal-secret'));
});
