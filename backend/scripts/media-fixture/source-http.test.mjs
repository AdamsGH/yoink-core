import test from 'node:test';
import assert from 'node:assert/strict';
import { SourceHttpFixture } from './source-http.mjs';

test('source fixture exposes bounded real playlist entries and an independent image', async t => {
  const playlistMedia = [Buffer.alloc(128, 1), Buffer.alloc(144, 2)];
  const imageBytes = Buffer.alloc(160, 3);
  const fixture = new SourceHttpFixture({ mediaBytes: playlistMedia[0], playlistMedia, imageBytes, cookieSecret: 'fixture' });
  const port = await fixture.start();
  t.after(() => fixture.close());
  const get = pathname => fetch(`http://127.0.0.1:${port}${pathname}`);
  const page = await get('/playlist.html?count=11&broken=2');
  const html = await page.text();
  assert.equal(page.headers.get('content-type'), 'text/html');
  assert.equal((html.match(/<video /g) ?? []).length, 11);
  assert(html.includes('/missing-2.mp4'));
  for (const index of [1, 2, 11, 21]) {
    const response = await get(`/entry-${index}.mp4`);
    assert.equal(response.headers.get('content-type'), 'video/mp4');
    assert.deepEqual(Buffer.from(await response.arrayBuffer()), playlistMedia[(index - 1) % 2]);
  }
  const image = await get('/image.png');
  assert.equal(image.headers.get('content-type'), 'image/png');
  assert.deepEqual(Buffer.from(await image.arrayBuffer()), imageBytes);
  for (const count of ['0', '22', '1.5', 'NaN']) assert.equal((await get(`/playlist.html?count=${count}`)).status, 400);
  for (const path of ['/entry-0.mp4', '/entry-22.mp4', '/entry-999999999999999999999.mp4', '/missing-2.mp4']) assert.equal((await get(path)).status, 404);
  assert.equal(fixture.error, undefined);
});
