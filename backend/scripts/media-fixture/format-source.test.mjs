import assert from 'node:assert/strict';
import { test } from 'node:test';
import { FormatSource } from './format-source.mjs';
import { parseFormatReport } from './format-scenarios.mjs';

const valid = 'Yoink source formats v1\nID\tEXT\tWIDTH\tHEIGHT\tFPS\tVIDEO\tAUDIO\tPROTOCOL\n300\tmp4\t1280\t720\t25\th264\taac\thls\n';
test('format report contract rejects free-form metadata and oversized numeric fields', () => {
  assert.equal(parseFormatReport(Buffer.from(valid))[0][3], '720');
  for (const report of [valid.replace('300\t', 'https://signed.secret/\t'), valid.replace('h264', 'cookie-secret'), valid.replace('1280', '99999'), valid.replace('25\t', '1001\t'), valid.trimEnd(), valid + '\n', valid.replace('Yoink', 'Unknown')]) assert.throws(() => parseFormatReport(Buffer.from(report)));
});
test('format source serves only manifests, enforces cookies, and records forbidden payload access', () => {
  const source = new FormatSource('test-cookie');
  function response(path, headers = {}) {
    const result = { status: null, body: '', destroyed: false, writeHead(status) { this.status = status; }, end(body) { this.body = body ?? ''; }, once() {} };
    assert(source.respond({ method: 'GET', headers }, result, new URL(path, 'http://fixture.invalid')));
    return result;
  }
  assert.equal(response('/format-fixture/master.m3u8').status, 200);
  assert.equal(response('/format-fixture/forbidden-payload.ts').status, 404);
  assert.equal(response('/format-fixture/protected/master.m3u8').status, 403);
  assert.equal(response('/format-fixture/protected/master.m3u8', { cookie: 'session=test-cookie' }).status, 200);
  const gated = response('/format-fixture/master.m3u8?gate=true');
  assert.equal(gated.status, null);
  source.release();
  assert.equal(gated.status, 200);
  assert.equal(source.gated.size, 0);
});
