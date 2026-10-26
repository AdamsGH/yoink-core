import assert from 'node:assert/strict';
import { test } from 'node:test';
import { publishedImage } from './registry-image.mjs';

const revision = 'a'.repeat(40);
const args = { image: 'ghcr.io/adamsgh/yoink-core/backend', tag: '0.1.4', revision, user: 'fixture', token: 'fixture' };
const response = (data, status = 200) => new Response(JSON.stringify(data), { status });

test('registry refuses authentication errors rather than treating them as missing images', async () => {
  await assert.rejects(publishedImage(args, async () => response({}, 403)), /authorization failed/);
  await assert.rejects(publishedImage({ ...args, image: 'elsewhere.invalid/owner/image' }), /Invalid immutable/);
});

test('only manifest 404 permits publication of a new immutable tag', async () => {
  for (const status of [404, 401, 429, 500]) {
    let calls = 0;
    const result = publishedImage(args, async () => ++calls === 1 ? response({ token: 'fixture' }) : response({}, status));
    if (status === 404) assert.equal(await result, false);
    else await assert.rejects(result, /manifest check failed/);
  }
});

test('existing OCI image indexes are reusable only for the exact source commit', async () => {
  for (const source of [revision, 'b'.repeat(40)]) {
    const replies = [
      { token: 'fixture' },
      { manifests: [{ digest: `sha256:${'1'.repeat(64)}`, platform: { os: 'linux', architecture: 'amd64' } }] },
      { config: { digest: `sha256:${'2'.repeat(64)}` } },
      { config: { Labels: { 'org.opencontainers.image.revision': source } } },
    ];
    const result = publishedImage(args, async () => response(replies.shift()));
    if (source === revision) assert.equal(await result, true);
    else await assert.rejects(result, /another commit/);
  }
});
