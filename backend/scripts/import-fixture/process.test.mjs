import assert from 'node:assert/strict';
import test from 'node:test';
import { randomUUID } from 'node:crypto';
import { run, assertNoSecrets } from './process.mjs';

const node = (code, options) => run(process.execPath, ['-e', code], options);

test('captures output and writes bounded command input', async () => {
  const result = await node("process.stdin.on('data', x => process.stdout.write(x)); process.stderr.write('diagnostic');", { input: 'fixture-input' });
  assert.equal(result.stdout, 'fixture-input');
  assert.equal(result.stderr, 'diagnostic');
});

test('reports asynchronous spawn failure without an unhandled error', async () => {
  await assert.rejects(run(`/nonexistent/yoink-${randomUUID()}`, [], { timeout: 1000 }), error => error.category === 'spawn');
});

test('nonzero exit preserves diagnostics outside the safe message', async () => {
  await assert.rejects(node("process.stdout.write('marker'); process.exit(7)"), error => {
    assert.equal(error.category, 'exit');
    assert.equal(error.stdout, 'marker');
    assert(!error.message.includes('marker'));
    return true;
  });
});

test('timeout reaps the child before rejection', async () => {
  await assert.rejects(node("console.log(process.pid); process.on('SIGTERM',()=>{}); setInterval(()=>{},1000)", { timeout: 300 }), error => {
    assert.equal(error.category, 'timeout');
    const pid = Number(error.stdout.trim());
    assert(pid > 0);
    assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' });
    return true;
  });
});

test('output overflow is bounded and escalates without waiting for the command timeout', async () => {
  const started = Date.now();
  await assert.rejects(node("process.on('SIGTERM',()=>{}); setInterval(()=>process.stdout.write('x'.repeat(65536)),1)", { timeout: 15_000 }), error => {
    assert.equal(error.category, 'output_limit');
    assert.equal(Buffer.byteLength(error.stdout) + Buffer.byteLength(error.stderr), 1024 * 1024);
    assert(Date.now() - started < 10_000);
    return true;
  });
});

test('secret assertion never echoes the rejected value', () => {
  assert.throws(() => assertNoSecrets('fake-secret', ['fake-secret']), error => !error.message.includes('fake-secret'));
});
