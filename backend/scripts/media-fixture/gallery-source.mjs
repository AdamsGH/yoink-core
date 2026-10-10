import assert from 'node:assert/strict';
import https from 'node:https';
import path from 'node:path';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { deflateSync } from 'node:zlib';

function chunk(type, payload) {
  const body = Buffer.concat([Buffer.from(type), payload]);
  let crc = 0xffffffff;
  for (const byte of body) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
  }
  const size = Buffer.alloc(4), checksum = Buffer.alloc(4);
  size.writeUInt32BE(payload.length);
  checksum.writeUInt32BE((crc ^ 0xffffffff) >>> 0);
  return Buffer.concat([size, body, checksum]);
}

export function galleryImage(index) {
  const width = 160, height = 120;
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width); header.writeUInt32BE(height, 4); header[8] = 8; header[9] = 2;
  const raw = Buffer.alloc(height * (width * 3 + 1));
  for (let row = 0; row < height; row++) for (let col = 0; col < width; col++) {
    const at = row * (width * 3 + 1) + 1 + col * 3;
    raw[at] = (index * 37) % 256; raw[at + 1] = (index * 71) % 256; raw[at + 2] = (index * 113) % 256;
  }
  return Buffer.concat([Buffer.from('89504e470d0a1a0a', 'hex'), chunk('IHDR', header), chunk('IDAT', deflateSync(raw)), chunk('IEND', Buffer.alloc(0))]);
}

// Exact pinned Imgbox HTML contracts. No upstream requests or custom extractors.
export function galleryResponse(requestPath) {
  const gallery = requestPath.match(/^\/g\/(Gallery|Missing)(\d{3})$/);
  if (gallery) {
    const count = Number(gallery[2]);
    if (count < 1 || count > 21) return { status: 404, body: '' };
    const key = gallery[1] + gallery[2];
    return { status: 200, type: 'text/html', body: `<h1>Fixture - ${count} images</h1>` + Array.from({ length: count }, (_, i) => `<a href="/item-${key}-${i + 1}"><img alt="fixture"></a>`).join('') };
  }
  const item = requestPath.match(/^\/item-((?:Gallery|Missing)\d{3})-(\d+)$/);
  if (item) {
    const index = Number(item[2]);
    return { status: 200, type: 'text/html', body: `</a> &nbsp; ${index} of 21 <div class="image-container"><img title="fixture-${index}.png"></div><meta property="og:image" content="https://imgbox.com/file-${item[1]}-${index}.png">` };
  }
  const image = requestPath.match(/^\/file-((?:Gallery|Missing)\d{3})-(\d+)\.png$/);
  if (image && !(image[1].startsWith('Missing') && Number(image[2]) === 3)) {
    return { status: 200, type: 'image/png', body: galleryImage(Number(image[2])) };
  }
  return { status: 404, body: '' };
}

export async function startGallerySource({ tempRoot, capture, helperContainer, image, providerResponse, extraHosts = [] }) {
  const directory = path.join(tempRoot, 'gallery-tls');
  await mkdir(directory, { mode: 0o700 });
  const cert = path.join(directory, 'server.crt'), key = path.join(directory, 'server.key');
  await capture('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=imgbox.com',
    '-addext', 'subjectAltName=DNS:imgbox.com,DNS:boards.4chan.org,DNS:a.4cdn.org,DNS:i.4cdn.org,DNS:instagram.com,DNS:www.instagram.com' + extraHosts.map(host => ',DNS:' + host).join(''), '-addext', 'basicConstraints=critical,CA:TRUE', '-keyout', key, '-out', cert]);
  const certificate = await readFile(cert);
  const trust = path.join(directory, 'trust.pem');
  await writeFile(trust, certificate, { mode: 0o644 });
  // Resolve the installed dependency rather than guessing its Python-version-specific path.
  const trustPaths = JSON.parse((await helperContainer(['--network', 'none', '--entrypoint', 'python', image, '-I', '-c',
    'import certifi,json; from curl_cffi import Curl,CurlInfo; c=Curl(); print(json.dumps([certifi.where(),c.getinfo(CurlInfo.CAINFO).decode()])); c.close()'], 15000)).trim());
  assert.equal(trustPaths.length, 2, 'fixture needs Python and impersonated HTTP trust stores');
  assert.match(trustPaths[0], /^\/usr\/local\/lib\/python3\.\d+\/site-packages\/certifi\/cacert\.pem$/);
  assert.match(trustPaths[1], /^\/etc\/ssl\/(?:[a-zA-Z0-9._-]+\/)*[a-zA-Z0-9._-]+\.crt$/);
  let requests = 0;
  const paths = new Set();
  let error;
  const server = https.createServer({ cert: certificate, key: await readFile(key) }, async (req, res) => {
    try {
    requests++;
    paths.add(req.url);
    const chunks = []; let bodyBytes = 0;
    for await (const chunk of req) { bodyBytes += chunk.length; assert(bodyBytes <= 1024 * 1024, 'bounded fixture metadata request'); chunks.push(chunk); }
    const body = Buffer.concat(chunks).toString('utf8');
    const response = providerResponse?.(req.headers.host, req.url, { headers: req.headers, method: req.method, body }) ?? (req.headers.host === 'imgbox.com' ? galleryResponse(req.url) : { status: 400, body: '' });
    if (response.gate) await response.gate;
    if (res.destroyed) return;
    res.writeHead(response.status, { 'content-type': response.type ?? 'text/plain', 'content-length': response.declaredLength ?? Buffer.byteLength(response.body), ...response.headers });
    if (response.truncateAt !== undefined) {
      res.write(response.body.subarray(0, response.truncateAt));
      // Let the installed downloader observe a successful partial write before EOF.
      setTimeout(() => res.destroy(), 30).unref();
    } else res.end(response.body);
    } catch (cause) { error = cause; res.destroy(); }
  });
  server.on('tlsClientError', cause => console.error('Gallery fixture TLS handshake failed:', cause.code));
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  return {
    get error() { return error; },
    get requests() { return requests; },
    paths,
    untrustedDockerArgs: ['--add-host', 'instagram.com:192.0.2.1', '--add-host', 'www.instagram.com:192.0.2.1', '--add-host', 'boards.4chan.org:192.0.2.1', '--add-host', 'a.4cdn.org:192.0.2.1', '--add-host', 'i.4cdn.org:192.0.2.1', '--add-host', 'imgbox.com:192.0.2.1'],
    port: server.address().port,
    dockerArgs: [...extraHosts.flatMap(host => ['--add-host', host + ':192.0.2.1']), '--add-host', 'instagram.com:192.0.2.1', '--add-host', 'www.instagram.com:192.0.2.1', '--add-host', 'boards.4chan.org:192.0.2.1', '--add-host', 'a.4cdn.org:192.0.2.1', '--add-host', 'i.4cdn.org:192.0.2.1', '--add-host', 'imgbox.com:192.0.2.1', ...trustPaths.flatMap(trustPath => ['--mount', `type=bind,source=${trust},target=${trustPath},readonly`])],
    async close() { server.closeAllConnections(); await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve())); },
  };
}
