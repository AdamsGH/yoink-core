import test from 'node:test';
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import { inflateSync } from 'node:zlib';
import net from 'node:net';
import { once } from 'node:events';
import { galleryImage, galleryResponse, startGallerySource } from './gallery-source.mjs';
import { AuthenticatedProxyFixture } from './source-http.mjs';

test('gallery fixture supplies ordered distinct small PNGs and explicit broken sources', () => {
  const html = galleryResponse('/g/Gallery012');
  assert.equal(html.status, 200);
  assert.equal((html.body.match(/<a href=/g) ?? []).length, 12);
  assert(galleryResponse('/item-Gallery012-12').body.includes('https://imgbox.com/file-Gallery012-12.png'));
  const hashes = new Set();
  for (let index = 1; index <= 21; index++) {
    const bytes = galleryImage(index);
    assert(bytes.length < 1024, 'small-image cache regression must stay below the old rejection threshold');
    assert.equal(bytes.subarray(0, 8).toString('hex'), '89504e470d0a1a0a');
    assert.equal(bytes.readUInt32BE(16), 160); assert.equal(bytes.readUInt32BE(20), 120);
    const length = bytes.readUInt32BE(33);
    assert.equal(bytes.subarray(37, 41).toString(), 'IDAT');
    const raw = inflateSync(bytes.subarray(41, 41 + length));
    assert.equal(raw.length, 120 * 481);
    assert.equal(raw[1], (index * 37) % 256);
    hashes.add(bytes.toString('hex'));
  }
  assert.equal(hashes.size, 21);
  assert.equal(galleryResponse('/file-Missing003-2.png').status, 200);
  assert.equal(galleryResponse('/file-Missing003-3.png').status, 404);
  for (const pathname of ['/g/Gallery000', '/g/Gallery022', '/g/Gallery999', '/not-a-gallery']) assert.equal(galleryResponse(pathname).status, 404);
});

test('proxy gallery mapping requires exact authority, opt-in and fixture authentication', async t => {
  const origin = net.createServer(socket => { socket.on('error', () => {}); });
  origin.listen(0, '127.0.0.1');
  await once(origin, 'listening');
  const proxy = new AuthenticatedProxyFixture({ originPort: origin.address().port, authorization: 'Basic fixture-only' });
  const port = await proxy.start();
  t.after(async () => { await proxy.close(); await new Promise(resolve => origin.close(resolve)); });
  const connect = async (authority, auth = 'Basic fixture-only') => {
    const socket = net.connect(port, '127.0.0.1');
    socket.setTimeout(2000, () => socket.destroy(new Error('proxy fixture response timeout')));
    try {
      await once(socket, 'connect');
      socket.write(`CONNECT ${authority} HTTP/1.1\r\nProxy-Authorization: ${auth}\r\n\r\n`);
      const [response] = await once(socket, 'data');
      return response.toString().split('\r\n')[0];
    } finally { socket.destroy(); }
  };
  assert.match(await connect('192.0.2.1:443'), /403/);
  proxy.galleryPort = origin.address().port;
  assert.match(await connect('192.0.2.1:443', 'wrong'), /407/);
  for (const authority of ['imgbox.com:443', '192.0.2.2:443', '192.0.2.1:80']) assert.match(await connect(authority), /403/);
  assert.match(await connect('192.0.2.1:443'), /200/);
  assert.equal(proxy.authHits, 1);
  proxy.galleryPort = undefined;
  assert.match(await connect('192.0.2.1:443'), /403/);
});

test('fixture trusts both Python and impersonated HTTP CA stores without trusting negative controls', async () => {
  const tempRoot = await mkdtemp(path.join(tmpdir(), 'gallery-trust-'));
  const paths = ['/usr/local/lib/python3.12/site-packages/certifi/cacert.pem', '/etc/ssl/certs/ca-certificates.crt'];
  const capture = promisify(execFile);
  let source;
  try {
    source = await startGallerySource({ tempRoot, image: 'fixture',
      capture: (command, args) => capture(command, args, { timeout: 10000 }),
      helperContainer: async (args) => {
        assert(args.at(-1).includes('CurlInfo.CAINFO'));
        return JSON.stringify(paths);
      },
    });
    const mounts = source.dockerArgs.filter(arg => arg.startsWith('type=bind,'));
    assert.equal(mounts.length, 2);
    paths.forEach((target, index) => assert(mounts[index].endsWith(`target=${target},readonly`)));
    assert.equal(source.untrustedDockerArgs.includes('--mount'), false);
  } finally {
    await source?.close();
    await rm(tempRoot, { recursive: true, force: true });
  }
});
