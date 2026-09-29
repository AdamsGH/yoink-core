import test from 'node:test';
import assert from 'node:assert/strict';
import { BotApiFixture } from './bot-api.mjs';

test('document updates and Telegram getFile/file transport return registered bytes without logging credentials', async t => {
  const api = new BotApiFixture({ mediaBytes: Buffer.alloc(8) });
  const port = await api.start();
  t.after(() => api.close());
  const bytes = Buffer.from('fixture cookie bytes');
  api.setDocumentFile('opaque-fixture-id', bytes, { filePath: 'documents/cookie.txt' });
  api.enqueueDocument(42, 42, '/cookie', 1_000_000, 'opaque-fixture-id', { filename: 'site.test.txt', language: 'ru' });
  const update = await fetch(`http://127.0.0.1:${port}/token/getUpdates`, {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}',
  }).then(response => response.json());
  assert.equal(update.result[0].message.caption, '/cookie');
  assert.equal(update.result[0].message.document.file_name, 'site.test.txt');
  assert.equal(update.result[0].message.from.language_code, 'ru');
  const metadata = await fetch(`http://127.0.0.1:${port}/token/getFile`, {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ file_id: 'opaque-fixture-id' }),
  }).then(response => response.json());
  assert.equal(metadata.result.file_path, 'documents/cookie.txt');
  const downloaded = await fetch(`http://127.0.0.1:${port}/file/token/${metadata.result.file_path}`);
  assert.deepEqual(Buffer.from(await downloaded.arrayBuffer()), bytes);
  assert.deepEqual(api.downloadCalls, [
    { kind: 'getFile', fileId: 'opaque-fixture-id' },
    { kind: 'file', matched: true },
  ]);
  assert(!JSON.stringify(api.calls).includes('opaque-fixture-id'));
});

test('document fixture can hold getFile and emit explicit path/HTTP failures and chunked oversize', async t => {
  const api = new BotApiFixture({ mediaBytes: Buffer.alloc(8) });
  const port = await api.start();
  t.after(() => api.close());
  api.setDocumentFile('held', Buffer.from('x'));
  const release = api.holdDocumentFile('held', 'getFile');
  const pending = fetch(`http://127.0.0.1:${port}/token/getFile`, {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ file_id: 'held' }),
  });
  await new Promise(resolve => setTimeout(resolve, 20));
  release();
  assert.equal((await pending).status, 200);
  api.setDocumentFile('oversize', Buffer.from('x'), { fileMode: 'chunked-oversize' });
  const oversize = await fetch(`http://127.0.0.1:${port}/file/token/documents/oversize.txt`);
  assert.equal(oversize.headers.get('content-length'), null);
  assert.equal((await oversize.arrayBuffer()).byteLength, 9 * 65536);
  api.setDocumentFile('failure', Buffer.from('x'), { getFileMode: 'failure' });
  const failure = await fetch(`http://127.0.0.1:${port}/token/getFile`, {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ file_id: 'failure' }),
  });
  assert.equal(failure.status, 500);
});

test('getFile preserves absolute adversarial paths so the adapter, not the fixture, enforces confinement', async t => {
  const remote = '/var/lib/telegram-bot-api/fixture-bot/documents';
  const api = new BotApiFixture({ mediaBytes: Buffer.alloc(8) });
  const port = await api.start();
  t.after(() => api.close());
  const cases = [
    `${remote}/valid.txt`, `${remote}/symlink.txt`, `${remote}/parent-link/valid.txt`,
    '/var/lib/telegram-bot-api/other-bot/documents/valid.txt', `${remote}/../outside.txt`,
    `${remote}/missing.txt`, `${remote}/directory`, `${remote}/oversize.txt`,
  ];
  for (const [index, remotePath] of cases.entries()) {
    const id = `absolute-${index}`;
    api.setDocumentFile(id, Buffer.from('synthetic'), { filePath: remotePath });
    const metadata = await fetch(`http://127.0.0.1:${port}/token/getFile`, {
      method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ file_id: id }),
    }).then(response => response.json());
    assert.equal(metadata.ok, true);
    assert.equal(metadata.result.file_path, remotePath);
  }
  assert.equal(api.downloadCalls.filter(call => call.kind === 'file').length, 0);
});
