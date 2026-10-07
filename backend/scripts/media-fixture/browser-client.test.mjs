import test from 'node:test';
import assert from 'node:assert/strict';
import { PassThrough } from 'node:stream';
import { access, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { createServer } from 'node:http';
import { CdpPipe, isAllowedRequest, launchBrowser } from './browser-client.mjs';

function transport() {
  const input = new PassThrough();
  const output = new PassThrough();
  const frames = [];
  output.on('data', chunk => frames.push(JSON.parse(chunk.toString().slice(0, -1))));
  const pipe = new CdpPipe(input, output, { commandTimeoutMs: 100 });
  return { input, output, frames, pipe };
}

async function absent(path) {
  await assert.rejects(access(path), { code: 'ENOENT' });
}

function processGone(pid) {
  assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' });
}

test('pipe frames fragmented UTF-8, coalesced replies and session events', async () => {
  const { pipe, input, frames } = transport();
  const first = pipe.request('Runtime.evaluate', { expression: 'secret' }, 'page-session');
  const second = pipe.request('Browser.getVersion');
  assert.equal(frames[0].sessionId, 'page-session');
  assert.equal(frames[1].sessionId, undefined);
  const events = [];
  pipe.on('event', event => events.push(event));
  const payload = Buffer.from(`${JSON.stringify({ id: 2, result: { version: 'second' } })}\0${JSON.stringify({ id: 1, result: { text: '☃' } })}\0${JSON.stringify({ method: 'Page.loadEventFired', sessionId: 'page-session', params: {} })}\0`);
  for (const byte of payload) input.write(Buffer.from([byte]));
  assert.deepEqual(await first, { text: '☃' });
  assert.deepEqual(await second, { version: 'second' });
  assert.equal(events[0].sessionId, 'page-session');
  assert.equal(pipe.pending.size, 0);
  pipe.fail();
});

test('CDP errors omit response bodies, params and credentials', async () => {
  const { pipe, input } = transport();
  const pending = pipe.request('Invalid.operation', { password: 'secret' });
  input.write(`${JSON.stringify({ id: 1, error: { code: -1, message: 'credential secret', data: 'response body' } })}\0`);
  await assert.rejects(pending, { message: 'CDP command failed: Invalid.operation' });
  pipe.fail();
});

test('command timeout clears pending state and ignores late responses', async () => {
  const { pipe, input } = transport();
  await assert.rejects(pipe.request('Runtime.evaluate', {}, 'session', { timeoutMs: 15 }), /timed out/);
  assert.equal(pipe.pending.size, 0);
  input.write('{"id":1,"result":{"late":true}}\0');
  const next = pipe.request('Browser.getVersion');
  input.write('{"id":2,"result":{}}\0');
  assert.deepEqual(await next, {});
  pipe.fail();
});

test('transport failure rejects all requests and all subsequent requests', async () => {
  const { pipe, input } = transport();
  const a = assert.rejects(pipe.request('Page.enable'), /CDP pipe ended/);
  const b = assert.rejects(pipe.request('Network.enable'), /CDP pipe ended/);
  input.end();
  await Promise.all([a, b]);
  await assert.rejects(pipe.request('Page.enable'), /CDP pipe ended/);
  assert.equal(pipe.pending.size, 0);
});

test('malformed frames and write failures have sanitized terminal errors', async () => {
  const first = transport();
  const pending = assert.rejects(first.pipe.request('Page.enable'), { message: 'Invalid CDP frame' });
  first.input.write('not json secret\0');
  await pending;
  const second = transport();
  const rejected = assert.rejects(second.pipe.request('Page.enable'), { message: 'CDP pipe failed' });
  second.output.emit('error', new Error('secret'));
  await rejected;
});

test('origin restriction handles redirects, ports and local URL schemes', () => {
  const origin = 'https://fixture.test:8443';
  for (const url of [`${origin}/api`, 'about:blank', 'about:srcdoc', 'data:text/html,hello', `blob:${origin}/123`]) {
    assert.equal(isAllowedRequest(url, origin), true, url);
  }
  for (const url of ['https://fixture.test/api', 'http://fixture.test:8443', 'https://fixture.test.evil:8443', 'file:///tmp/private', 'ws://fixture.test:8443', 'about:settings', 'not a URL']) {
    assert.equal(isAllowedRequest(url, origin), false, url);
  }
});

test('input validation and spawn failure do not leak profiles', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'yoink-browser-startup-'));
  try {
    const options = { origin: 'https://fixture.test', profileParentDirectory: directory };
    await assert.rejects(launchBrowser({ ...options, origin: 'https://fixture.test/path' }), /exact HTTP/);
    await assert.rejects(launchBrowser({ ...options, certificateSpki: 'secret' }), /SPKI/);
    await assert.rejects(launchBrowser({ ...options, startupTimeoutMs: 0 }), /startup timeout/);
    await assert.rejects(launchBrowser({ ...options, profileParentDirectory: 'relative' }), /absolute profile parent/);
    await assert.rejects(launchBrowser({ ...options, executablePath: '/definitely/missing/chromium' }), /Browser launch failed/);
    assert.deepEqual(await readdir(directory), []);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('bounded startup failure removes its owned profile', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'yoink-browser-test-'));
  try {
    const executablePath = join(directory, 'silent-browser');
    const recordFile = join(directory, 'record.json');
    await writeFile(executablePath, `#!${process.execPath}\nrequire('node:fs').writeFileSync(${JSON.stringify(recordFile)}, JSON.stringify({ pid: process.pid, args: process.argv.slice(2) }));\nprocess.on('SIGTERM', () => {});\nsetInterval(() => {}, 1000);\n`, { mode: 0o700 });
    const certificateSpki = `${'A'.repeat(43)}=`;
    await assert.rejects(launchBrowser({ origin: 'https://fixture.test', executablePath, certificateSpki, startupTimeoutMs: 500, shutdownTimeoutMs: 50 }), /startup timed out/);
    const record = JSON.parse(await readFile(recordFile, 'utf8'));
    processGone(record.pid);
    assert.equal(record.args.includes('--remote-debugging-pipe'), true);
    assert.equal(record.args.some(arg => arg.startsWith('--remote-debugging-port')), false);
    assert.equal(record.args.includes('--no-sandbox'), false);
    assert.equal(record.args.includes('--disable-setuid-sandbox'), false);
    assert.equal(record.args.includes('--disable-background-networking'), true);
    assert.equal(record.args.includes(`--ignore-certificate-errors-spki-list=${certificateSpki}`), true);
    assert.equal(record.args.includes('--ignore-certificate-errors'), false);
    const profile = record.args.find(arg => arg.startsWith('--user-data-dir=')).slice('--user-data-dir='.length);
    await absent(profile);
    await access(executablePath); // A sibling directory is not owned by the client.
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('Chromium about:blank smoke: attached session, bootstrap, invalid operation and timeout', { timeout: 30000 }, async () => {
  const browser = await launchBrowser({ origin: 'https://fixture.test', bootstrapScript: 'globalThis.fixtureBootstrap = "installed";' });
  try {
    assert.ok(browser.sessionId);
    assert.equal(await browser.evaluate('location.href'), 'about:blank');
    await browser.navigate('data:text/html,<title>fixture</title>');
    assert.equal(await browser.evaluate('fixtureBootstrap'), 'installed');
    assert.equal(await browser.evaluate('document.title'), 'fixture');
    const directory = await mkdtemp(join(tmpdir(), 'yoink-upload-test-'));
    try {
      const file = join(directory, 'cookies.txt');
      await writeFile(file, '# fixture cookies\n');
      await browser.evaluate('document.body.innerHTML = \'<input type="file" id="upload">\'');
      const { root } = await browser.request('DOM.getDocument');
      const { nodeId } = await browser.request('DOM.querySelector', { nodeId: root.nodeId, selector: '#upload' });
      await browser.request('DOM.setFileInputFiles', { nodeId, files: [file] });
      assert.equal(await browser.evaluate('document.querySelector("#upload").files[0].text()'), '# fixture cookies\n');
    } finally { await rm(directory, { recursive: true, force: true }); }
    await assert.rejects(browser.request('Fixture.invalidOperation'), /CDP command failed/);
    await assert.rejects(browser.evaluate('new Promise(() => {})', { timeoutMs: 25 }), /timed out/);
    assert.equal(await browser.evaluate('6 * 7'), 42);
    await assert.rejects(browser.evaluate('throw new Error("secret")'), { message: 'Browser evaluation failed' });
    await assert.rejects(browser.navigate('https://outside.test'), /outside fixture origin/);
  } finally { await browser.close(); }
  await browser.close();
  await absent(browser.profileDirectory);
  processGone(browser.pid);
});

test('Chromium process failure rejects pending requests and automatically cleans up', { timeout: 30000 }, async () => {
  const browser = await launchBrowser({ origin: 'https://fixture.test' });
  const pending = assert.rejects(browser.evaluate('new Promise(() => {})'), /Browser exited|CDP pipe ended|CDP pipe failed/);
  process.kill(browser.pid, 'SIGKILL');
  await pending;
  // Verify cleanup is automatic, not merely a consequence of calling close().
  const deadline = Date.now() + 5000;
  while (true) {
    try { await access(browser.profileDirectory); }
    catch (error) { if (error.code === 'ENOENT') break; throw error; }
    assert.ok(Date.now() < deadline, 'automatic browser profile cleanup timed out');
    await new Promise(resolve => setTimeout(resolve, 10));
  }
  await browser.close();
  await absent(browser.profileDirectory);
  processGone(browser.pid);
});

async function server(handler) {
  const http = createServer(handler);
  await new Promise(resolve => http.listen(0, '127.0.0.1', resolve));
  return { http, origin: `http://127.0.0.1:${http.address().port}`, close: () => new Promise(resolve => http.close(resolve)) };
}

test('Chromium permits real same-origin requests and blocks cross-origin requests and redirects', { timeout: 30000 }, async () => {
  let outsideHits = 0;
  const outside = await server((request, response) => { outsideHits++; response.end('outside'); });
  outside.http.on('upgrade', (request, socket) => { outsideHits++; socket.destroy(); });
  const fixture = await server((request, response) => {
    if (request.url === '/redirect') { response.writeHead(302, { location: `${outside.origin}/redirected` }); response.end(); }
    else if (request.url === '/worker.js') {
      response.setHeader('Content-Type', 'text/javascript');
      response.end(`onmessage = async () => { const same = await fetch('/api').then(r => r.text()); const blocked = await fetch(${JSON.stringify(outside.origin)}).then(() => false, () => true); postMessage({ same, blocked }); };`);
    }
    else if (request.url === '/api') { response.end('real fixture response'); }
    else { response.setHeader('Content-Type', 'text/html'); response.end('<title>fixture</title>'); }
  });
  let browser;
  try {
    browser = await launchBrowser({ origin: fixture.origin });
    await browser.navigate(fixture.origin);
    assert.equal(await browser.evaluate('fetch("/api").then(r => r.text())'), 'real fixture response');
    assert.equal(await browser.evaluate(`fetch(${JSON.stringify(outside.origin)}).then(() => false, () => true)`), true);
    assert.equal(await browser.evaluate('fetch("/redirect").then(() => false, () => true)'), true);
    assert.deepEqual(await browser.evaluate(`new Promise((resolve, reject) => {
      const worker = new Worker('/worker.js');
      worker.onmessage = event => { worker.terminate(); resolve(event.data); };
      worker.onerror = reject;
      worker.postMessage('start');
    })`), { same: 'real fixture response', blocked: true });
    assert.equal(await browser.evaluate(`(() => {
      try { new WebSocket(${JSON.stringify(outside.origin.replace('http:', 'ws:'))}); return false; }
      catch { return true; }
    })()`), true);
    assert.equal(outsideHits, 0);
  } finally {
    await browser?.close();
    await Promise.all([fixture.close(), outside.close()]);
  }
});
