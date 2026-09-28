import test from 'node:test';
import assert from 'node:assert/strict';
import { AdaptiveSource } from './adaptive-source.mjs';
import { SourceHttpFixture } from './source-http.mjs';

test('adaptive fixture serves exact bytes and isolates fragment/key failures by URL path', async t => {
  const transportStream = Buffer.alloc(188, 71);
  const files = new Map([
    ['hls/index.m3u8', Buffer.from('#EXTM3U\n#EXTINF:1.0,\nsegment-00.ts\n#EXTINF:1.0,\nsegment-01.ts\n#EXT-X-ENDLIST\n')],
    ['hls/segment-00.ts', transportStream], ['hls/segment-01.ts', transportStream],
    ['dash/chunk-0-00002.m4s', Buffer.from('video segment')],
  ]);
  const source = new SourceHttpFixture({ mediaBytes: Buffer.alloc(128), cookieSecret: 'fixture' });
  source.adaptiveSource = new AdaptiveSource(files);
  const port = await source.start();
  t.after(() => source.close());
  const get = (path, method = 'GET') => fetch(`http://127.0.0.1:${port}/adaptive/${path}`, { method, redirect: 'manual' });
  for (const [name, bytes] of files) assert.deepEqual(Buffer.from(await (await get(`good/${name}`)).arrayBuffer()), bytes);
  assert.equal((await get('good/hls/segment-00.ts', 'HEAD')).headers.get('content-length'), String(transportStream.length));
  for (const name of ['hls/segment-01.ts', 'dash/chunk-0-00002.m4s']) {
    assert.equal((await get(`missing/${name}`)).status, 404);
    const redirect = await get(`private-redirect/${name}`);
    assert.equal(redirect.status, 302);
    assert.equal(redirect.headers.get('location'), 'http://169.254.169.254/blocked-segment');
  }
  const keyManifest = await (await get('private-key/hls/index.m3u8')).text();
  assert(keyManifest.includes('METHOD=AES-128,URI="http://169.254.169.254/blocked-key"'));
  assert.equal((await get('missing/hls/segment-00.ts')).status, 200, 'only the middle segment is missing');
  assert.equal((await get('unknown/hls/segment-00.ts')).status, 404);
  assert.equal((await get('good/hls/not-generated.ts')).status, 404);
  assert.equal(source.error, undefined);
  assert(source.adaptiveSource.requests.some(r => r.method === 'HEAD'));
});
