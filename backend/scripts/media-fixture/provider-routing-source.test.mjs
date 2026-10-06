import test from 'node:test';
import assert from 'node:assert/strict';
import { INSTAGRAM_CASES, ProviderRoutingSource, instagramMediaId, providerRoutingResponse } from './provider-routing-source.mjs';

const video = Buffer.from('synthetic gallery video bytes');
const primary = Buffer.alloc(4096, 7);
const cookieSecret = 'test-only-session-value';
const apiPath = code => `/api/v1/media/${instagramMediaId(code)}/info/`;
const request = provider => ({ headers: { 'x-asbd-id': provider === 'primary' ? '359341' : '129477', cookie: `sessionid=${cookieSecret}` }, method: 'GET' });
const source = () => new ProviderRoutingSource({ mediaBytes: video, primaryBytes: primary, cookieSecret });

test('pinned 4chan fixture supplies ordered mixed media and a missing second output', () => {
  const response = (host, path) => providerRoutingResponse(video, host, path);
  const posts = JSON.parse(response('a.4cdn.org', '/test/thread/7001.json').body).posts;
  assert.deepEqual(posts.map(p => p.ext), ['.png', '.mp4']);
  assert.equal(response('i.4cdn.org', '/test/70011.png').type, 'image/png');
  assert.equal(response('i.4cdn.org', '/test/70012.mp4').body, video);
  assert.equal(response('i.4cdn.org', '/test/70022.mp4').status, 404);
  assert.equal(response('evil.test', '/test/thread/7001.json'), undefined);
  assert.equal(response('a.4cdn.org', '/test/thread/7001.json?spoof=1').status, 404);
});

test('pinned Instagram response shapes distinguish real library headers and require session cookies', () => {
  const fixture = source();
  assert.equal(instagramMediaId('B'), '1');
  assert.equal(instagramMediaId('BA'), '64');
  assert.equal(new Set(Object.keys(INSTAGRAM_CASES).map(instagramMediaId)).size, Object.keys(INSTAGRAM_CASES).length);
  const api = (code, provider) => fixture.response('www.instagram.com', apiPath(code), request(provider));
  assert.equal(api('PRMeta', 'primary').status, 404);
  assert.equal(api('PRJson', 'primary').status, 200);
  assert.throws(() => JSON.parse(api('PRJson', 'primary').body), SyntaxError);
  assert.equal(JSON.parse(api('PRJson', 'gallery').body).items[0].carousel_media.length, 2);
  const fallback = JSON.parse(api('PRMeta', 'gallery').body).items[0];
  assert.equal(fallback.code, 'PRMeta');
  assert.equal(fallback.caption.text, 'ordinary gallery evidence');
  assert.equal(fallback.carousel_media.length, 2);
  assert.equal(fallback.carousel_media[0].media_type, 1);
  assert.equal(fallback.carousel_media[1].media_type, 2);
  assert.equal(api('PRBoth', 'primary').status, 404);
  assert.equal(api('PRBoth', 'gallery').status, 404);
  const successfulPrimary = JSON.parse(api('PRSuccess', 'primary').body).items[0];
  assert.equal(successfulPrimary.video_versions.length, 1);
  assert.match(successfulPrimary.caption.text, /primary-only/);
  assert.match(JSON.parse(api('PRNsfw', 'gallery').body).items[0].caption.text, /sensitivecontent/);
  assert.equal(JSON.parse(api('PRItems', 'gallery').body).items[0].carousel_media.length, 21);
  assert.equal(fixture.response('www.instagram.com', apiPath('PRMeta'), { headers: {} }).status, 403);
  assert.equal(fixture.response('www.instagram.com', apiPath('PRMeta'), { headers: { ...request('gallery').headers, cookie: 'sessionid=wrong' } }).status, 403);
  assert(!JSON.stringify(fixture.trace).includes(cookieSecret), 'diagnostics never retain cookie/header values');
});

test('download-failure, budget and confinement fixtures are actual finite HTTP boundaries', () => {
  const fixture = source();
  const response = (code, file) => fixture.response('imgbox.com', `/routing/${code}/${file}`);
  const truncated = response('PRDownload', 'primary.mp4');
  assert.equal(truncated.body, primary);
  assert.equal(truncated.truncateAt, 2048);
  assert.equal(response('PRDownload', 'gallery.mp4').body, video);
  assert.equal(response('PRDownload', 'gallery-1.png').type, 'image/png');
  assert.equal(response('PRLimit', 'primary.mp4').declaredLength, 65537);
  assert.equal(response('PRRedirect', 'primary.mp4').headers.location, 'http://127.0.0.1:1/forbidden.mp4');
  assert.equal(response('PRSuccess', 'primary.mp4').body, primary);
  assert.equal(fixture.response('evil.test', '/routing/PRDownload/gallery.mp4'), undefined);
});

test('test-only gates fence exact provider and final-video stages without replacing extractors', async () => {
  const fixture = source();
  const release = fixture.hold('PRCookie', 'gallery-media');
  const response = fixture.response('imgbox.com', '/routing/PRCookie/gallery.mp4');
  assert(response.gate instanceof Promise);
  assert.equal(fixture.response('imgbox.com', '/routing/PRCookie/gallery-1.png').gate, undefined);
  let completed = false;
  response.gate.then(() => { completed = true; });
  await Promise.resolve();
  assert.equal(completed, false);
  release();
  await response.gate;
  assert.equal(completed, true);
  assert.equal(fixture.response('imgbox.com', '/routing/PRCookie/gallery.mp4').gate, undefined);
  const abortRelease = fixture.hold('PRAbort', 'primary-media');
  assert(fixture.response('imgbox.com', '/routing/PRAbort/primary.mp4').gate instanceof Promise);
  abortRelease();
  const apiRelease = fixture.hold('PRCancel', 'gallery-api');
  assert(fixture.response('www.instagram.com', apiPath('PRCancel'), request('gallery')).gate instanceof Promise);
  fixture.releaseAll();
  apiRelease();
});
