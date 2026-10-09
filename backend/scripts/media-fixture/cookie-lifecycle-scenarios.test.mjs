import assert from 'node:assert/strict';
import http from 'node:http';
import { test } from 'node:test';
import { concurrentCookieSubmit } from './cookie-lifecycle-scenarios.mjs';

test('concurrentCookieSubmit holds two separate public requests before releasing bodies', async () => {
  const sockets = new Set();
  const pending = [];
  const received = [];
  let consumed = false;
  const admitted = [];
  const server = http.createServer((req, res) => {
    assert.equal(req.method, 'POST');
    assert.equal(req.url, '/api/v1/dl/cookies/submit');
    assert.equal(req.headers.authorization, undefined);
    sockets.add(req.socket);
    let bytes = '';
    req.on('data', chunk => {
      bytes += chunk;
      if (!pending.includes(req)) pending.push(req);
    });
    req.on('end', () => {
      received.push(JSON.parse(bytes));
      assert.equal(admitted.length, 2, 'both native body readers admitted before submit completion');
      const status = consumed ? 401 : 204;
      consumed = true;
      setImmediate(() => { res.writeHead(status); res.end(); });
    });
  });
  server.on('checkContinue', (req, res) => {
    admitted.push({ req, res });
    server.emit('request', req, res);
    if (admitted.length === 2) admitted.forEach(peer => peer.res.writeContinue());
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try {
    const payloads = [{ token: 'synthetic-only', cookies: { a: [] } }, { token: 'synthetic-only', cookies: { b: [] } }];
    const result = await concurrentCookieSubmit(`http://127.0.0.1:${server.address().port}`, payloads);
    assert.deepEqual(result.slice().sort((a, b) => a - b), [204, 401]);
    assert.equal(sockets.size, 2);
    assert.equal(pending.length, 2);
    assert.deepEqual(received.map(v => JSON.stringify(v)).sort(), payloads.map(v => JSON.stringify(v)).sort());
  } finally { await new Promise(resolve => server.close(resolve)); }
});

test('concurrentCookieSubmit rejects non-loopback and non-HTTP boundaries before I/O', async () => {
  for (const base of ['https://127.0.0.1:1', 'http://example.invalid', 'http://127.0.0.1:1/path', 'http://user@127.0.0.1:1', 'http://127.0.0.1:1/?redirect=x']) {
    await assert.rejects(concurrentCookieSubmit(base, [{}, {}]), /native loopback HTTP boundary/);
  }
  await assert.rejects(concurrentCookieSubmit('http://127.0.0.1:1', [{}]), /two concurrent contenders/);
});

test('concurrentCookieSubmit returns errors after both local body readers are admitted', async () => {
  const server = http.createServer((req, res) => { req.resume(); req.on('end', () => { res.writeHead(503); res.end(); }); });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try {
    const result = await concurrentCookieSubmit(`http://127.0.0.1:${server.address().port}`, [{ token: 'x' }, { token: 'x' }]);
    assert.deepEqual(result, [503, 503]);
  } finally { await new Promise(resolve => server.close(resolve)); }
});

test('concurrentCookieSubmit destroys both sockets on an early final response', async () => {
  const server = http.createServer();
  server.on('checkContinue', (req, res) => { res.writeHead(503); res.end(); });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try {
    await assert.rejects(concurrentCookieSubmit(`http://127.0.0.1:${server.address().port}`, [{ token: 'x' }, { token: 'x' }]), /premature response/);
  } finally { await new Promise(resolve => server.close(resolve)); }
});
