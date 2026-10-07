import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { createHash, createPublicKey } from 'node:crypto';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { createServer } from 'node:https';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { promisify } from 'node:util';
import { launchBrowser } from './browser-client.mjs';

const execute = promisify(execFile);

test('browser trusts only the supplied fixture certificate fingerprint', { timeout: 60000 }, async () => {
  const directory = await mkdtemp(path.join(tmpdir(), 'yoink-browser-tls-'));
  let server, trusted, untrusted;
  try {
    const keyPath = path.join(directory, 'key.pem');
    const certPath = path.join(directory, 'cert.pem');
    await execute('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=localhost', '-addext', 'subjectAltName=IP:127.0.0.1', '-keyout', keyPath, '-out', certPath], { timeout: 15000, maxBuffer: 65536 });
    const cert = await readFile(certPath);
    const fingerprint = createHash('sha256').update(createPublicKey(cert).export({ type: 'spki', format: 'der' })).digest('base64');
    server = createServer({ key: await readFile(keyPath), cert }, (_request, response) => {
      response.writeHead(200, { 'Content-Type': 'text/html' });
      response.end('<!doctype html><title>Fixture HTTPS</title><main>fixture TLS ready</main>');
    });
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    const origin = `https://127.0.0.1:${server.address().port}`;
    trusted = await launchBrowser({ origin, certificateSpki: fingerprint });
    await trusted.navigate(origin);
    assert.equal(await trusted.evaluate('document.querySelector("main").textContent'), 'fixture TLS ready');
    assert.equal(await trusted.evaluate('window.isSecureContext'), true);
    await trusted.close();
    trusted = null;
    untrusted = await launchBrowser({ origin, certificateSpki: Buffer.alloc(32).toString('base64') });
    await assert.rejects(untrusted.navigate(origin), /Browser navigation failed/);
  } finally {
    const closed = await Promise.allSettled([trusted?.close(), untrusted?.close()]);
    if (server) await new Promise(resolve => server.close(resolve));
    await rm(directory, { recursive: true, force: true });
    const errors = closed.filter(result => result.status === 'rejected').map(result => result.reason);
    if (errors.length) throw new AggregateError(errors, 'Browser HTTPS cleanup failed');
  }
});
