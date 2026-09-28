import test from 'node:test';
import assert from 'node:assert/strict';
import { BotApiFixture } from './bot-api.mjs';
import { SourceHttpFixture } from './source-http.mjs';
import { subtitleBytes } from './subtitle-source.mjs';

test('subtitle source exposes exact tracks, missing files and chunked limits without video fetches', async t => {
  const source = new SourceHttpFixture({ mediaBytes: Buffer.alloc(128), cookieSecret: 'fixture' });
  const port = await source.start();
  t.after(() => source.close());
  const get = path => fetch(`http://127.0.0.1:${port}${path}`, { redirect: 'manual' });
  const page = await (await get('/subtitles.html?tracks=ru,ru-orig,en&format=srt')).text();
  assert.equal((page.match(/<track /g) ?? []).length, 3);
  assert(page.includes('/subtitles-ru-orig.srt'));
  for (const extension of ['vtt', 'srt']) {
    assert.deepEqual(Buffer.from(await (await get(`/subtitles-ru.${extension}`)).arrayBuffer()), subtitleBytes('ru', extension));
  }
  assert.equal((await get('/subtitles-ru-orig.vtt?case=missing')).status, 404);
  const oversized = await get('/subtitles-en.vtt?case=oversized');
  assert.equal(oversized.headers.get('content-length'), null);
  assert.equal(oversized.headers.get('transfer-encoding'), 'chunked');
  assert.equal((await oversized.arrayBuffer()).byteLength, 4 * 1024 * 1024 + 1);
  const redirect = await get('/subtitles-en.vtt?case=private-redirect');
  assert.equal(redirect.status, 302);
  assert.equal(redirect.headers.get('location'), 'http://169.254.169.254/blocked-subtitle');
  assert.equal(source.subtitleVideoRequests, 0);
});

for (const album of [false, true]) {
  test(`subtitle ${album ? 'album' : 'document'} fixture accepts small nonempty text and rejects zero bytes`, async t => {
    for (const empty of [false, true]) {
      const api = new BotApiFixture({ mediaBytes: Buffer.alloc(128) });
      const port = await api.start();
      t.after(() => api.close());
      const items = (album ? ['ru', 'ru-orig'] : ['en']).map(code => ({ kind: 'document', filename: `subtitles-${code}.vtt`, mimeType: 'text/vtt', bytes: empty ? Buffer.alloc(0) : subtitleBytes(code) }));
      const form = new FormData(); form.set('chat_id', '42');
      if (album) {
        api.setExpectedAlbum(items);
        form.set('media', JSON.stringify(items.map((_, i) => ({ type: 'document', media: `attach://file${i}` }))));
        items.forEach((item, i) => form.set(`file${i}`, new Blob([item.bytes], { type: item.mimeType }), item.filename));
      } else {
        api.setExpectedUpload({ method: 'senddocument', filename: items[0].filename, mimeType: 'text/vtt', preserveMdat: false });
        form.set('document', new Blob([items[0].bytes], { type: 'text/vtt' }), items[0].filename);
      }
      const post = () => fetch(`http://127.0.0.1:${port}/bottoken/${album ? 'sendMediaGroup' : 'sendDocument'}`, { method: 'POST', body: form });
      if (empty) {
        await assert.rejects(post());
        assert.match(api.error.message, /bounds/);
      } else {
        assert.equal((await (await post()).json()).ok, true);
        assert.deepEqual(album ? api.lastGroupBytes : [api.lastUploadedBytes], items.map(item => item.bytes));
        assert.equal(api.error, undefined);
      }
    }
  });
}
