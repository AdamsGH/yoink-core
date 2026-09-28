import http from 'node:http';
import net from 'node:net';

export class SourceHttpFixture {
  #server;
  #error;
  #slow = new Set();
  #gated = new Set();
  #requestTimes = [];
  #mediaRequests = 0;
  #protectedRequests = 0;
  #revokedQueued = 0;
  #lateProxyQueued = 0;
  #gateOpen = false;
  constructor({ mediaBytes, cookieSecret, playlistMedia = [mediaBytes], imageBytes }) {
    this.mediaBytes = mediaBytes;
    this.cookieSecret = cookieSecret;
    this.playlistMedia = playlistMedia;
    this.imageBytes = imageBytes;
  }
  get error() { return this.#error; }
  get mediaRequests() { return this.#mediaRequests; }
  get protectedRequests() { return this.#protectedRequests; }
  get revokedQueuedFetches() { return this.#revokedQueued; }
  get lateProxyQueuedFetches() { return this.#lateProxyQueued; }
  get requestTimes() { return this.#requestTimes; }
  get slowResponses() { return this.#slow; }
  get gatedResponses() { return this.#gated; }
  openGate() { this.#gateOpen = true; }
  closeGate() { this.#gateOpen = false; }
  destroySlowResponses() { for (const response of this.#slow) response.destroy(); }
  releaseGatedResponses() {
    this.#gateOpen = true;
    for (const response of this.#gated) {
      response.writeHead(200, { 'content-type': 'video/mp4', 'content-length': this.mediaBytes.length });
      response.end(this.mediaBytes);
    }
  }
  async start() {
    this.#server = http.createServer((req, res) => {
      this.#mediaRequests++;
      this.#requestTimes.push(Date.now());
      if (req.url.includes('case=revoked-queued')) this.#revokedQueued++;
      if (req.url.includes('case=late-proxy-queued')) this.#lateProxyQueued++;
      const url = new URL(req.url, 'http://fixture.invalid');
      if (url.pathname === '/playlist.html') {
        const count = Number(url.searchParams.get('count') ?? 2);
        if (!Number.isInteger(count) || count < 1 || count > 21) { res.writeHead(400); res.end(); return; }
        const broken = Number(url.searchParams.get('broken'));
        const videos = Array.from({ length: count }, (_, i) => `<video controls src="/${i + 1 === broken ? 'missing' : 'entry'}-${i + 1}.mp4"></video>`).join('\n');
        res.writeHead(200, { 'content-type': 'text/html' });
        res.end(`<html><head><title>Fixture playlist</title></head><body>${videos}</body></html>`);
        return;
      }
      const entry = url.pathname.match(/^\/entry-(\d+)\.mp4$/);
      if (entry) {
        const index = Number(entry[1]);
        if (!Number.isSafeInteger(index) || index < 1 || index > 21) { res.writeHead(404); res.end(); return; }
        const bytes = this.playlistMedia[(index - 1) % this.playlistMedia.length];
        res.writeHead(200, { 'content-type': 'video/mp4', 'content-length': bytes.length });
        res.end(bytes);
        return;
      }
      if (url.pathname === '/image.png' && this.imageBytes) {
        res.writeHead(200, { 'content-type': 'image/png', 'content-length': this.imageBytes.length });
        res.end(this.imageBytes);
        return;
      }
      if (req.url.startsWith('/gated')) {
        if (!this.#gateOpen) {
          this.#gated.add(res);
          res.once('close', () => this.#gated.delete(res));
          return;
        }
        this.#sendMedia(res);
        return;
      }
      if (req.url.startsWith('/slow')) {
        this.#slow.add(res);
        res.on('close', () => this.#slow.delete(res));
        return;
      }
      if (req.url.startsWith('/transient')) {
        res.writeHead(503, { 'content-type': 'text/plain' });
        res.end('temporary source unavailable');
        return;
      }
      if (req.url.startsWith('/cookie-media')) {
        this.#protectedRequests++;
        if (req.headers.cookie !== `fixture_auth=${this.cookieSecret}` || req.headers['proxy-authorization'] !== undefined) {
          this.#error ??= new Error('protected origin did not receive only the selected cookie');
          res.destroy(this.#error);
          return;
        }
        this.#sendMedia(res);
      } else if (req.url.startsWith('/media')) {
        this.#sendMedia(res);
      } else {
        res.writeHead(404);
        res.end();
      }
    });
    await new Promise((resolve, reject) => {
      this.#server.once('error', reject);
      this.#server.listen(0, '127.0.0.1', () => resolve());
    });
    return this.#server.address().port;
  }
  async close() {
    if (!this.#server) return;
    this.#server.closeAllConnections?.();
    await new Promise((resolve, reject) => this.#server.close(error => error ? reject(error) : resolve()));
  }
  #sendMedia(res) {
    res.writeHead(200, { 'content-type': 'video/mp4', 'content-length': this.mediaBytes.length });
    res.end(this.mediaBytes);
  }
}

export class AuthenticatedProxyFixture {
  #server;
  #sockets = new Set();
  #hits = 0;
  #authHits = 0;
  constructor({ originPort, authorization }) {
    this.originPort = originPort;
    this.authorization = authorization;
  }
  get hits() { return this.#hits; }
  get authHits() { return this.#authHits; }
  async start() {
    this.#server = net.createServer(client => {
      this.#sockets.add(client);
      client.once('close', () => this.#sockets.delete(client));
      let head = Buffer.alloc(0);
      let upstream;
      const closePair = () => { client.destroy(); upstream?.destroy(); };
      client.once('error', closePair);
      client.once('close', () => upstream?.destroy());
      const onData = chunk => {
        const bufferedLength = Math.min(chunk.length, 16 * 1024 - head.length);
        head = Buffer.concat([head, chunk.subarray(0, bufferedLength)]);
        const end = head.indexOf('\r\n\r\n');
        if (end < 0 || end + 4 > 16 * 1024) {
          client.removeListener('data', onData);
          client.end('HTTP/1.1 431 Request Header Fields Too Large\r\nContent-Length: 0\r\n\r\n');
          return;
        }
        client.removeListener('data', onData);
        const header = head.subarray(0, end + 4).toString('latin1');
        const [requestLine, ...headers] = header.split('\r\n');
        const match = requestLine.match(/^CONNECT ([^ ]+) HTTP\/1\.[01]$/);
        const authorization = headers.find(line => /^proxy-authorization:/i.test(line))?.split(':', 2)[1]?.trim();
        const permittedTarget = `127.0.0.1:${this.originPort}`;
        if (!match || match[1] !== permittedTarget) { client.end('HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n'); return; }
        if (authorization !== this.authorization) { client.end('HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm="fixture"\r\nContent-Length: 0\r\n\r\n'); return; }
        this.#hits++;
        this.#authHits++;
        upstream = net.connect(this.originPort, '127.0.0.1');
        this.#sockets.add(upstream);
        upstream.once('close', () => this.#sockets.delete(upstream));
        upstream.once('connect', () => {
          client.write('HTTP/1.1 200 Connection Established\r\n\r\n');
          const bufferedRemainder = head.subarray(end + 4);
          const chunkRemainder = chunk.subarray(bufferedLength);
          if (bufferedRemainder.length) upstream.write(bufferedRemainder);
          if (chunkRemainder.length) upstream.write(chunkRemainder);
          client.pipe(upstream);
          upstream.pipe(client);
        });
        upstream.once('error', closePair);
        upstream.once('close', () => client.destroy());
      };
      client.on('data', onData);
    });
    await new Promise((resolve, reject) => {
      this.#server.once('error', reject);
      this.#server.listen(0, '127.0.0.1', () => resolve());
    });
    return this.#server.address().port;
  }
  async close() {
    for (const socket of this.#sockets) socket.destroy();
    if (!this.#server) return;
    this.#server.closeAllConnections?.();
    await new Promise((resolve, reject) => this.#server.close(error => error ? reject(error) : resolve()));
  }
}

export async function freeLoopbackPort() {
  const listener = net.createServer();
  await new Promise((resolve, reject) => listener.listen(0, '127.0.0.1', error => error ? reject(error) : resolve()));
  const port = listener.address().port;
  await new Promise((resolve, reject) => listener.close(error => error ? reject(error) : resolve()));
  return port;
}
