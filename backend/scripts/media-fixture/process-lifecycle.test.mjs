import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { createContainerLifecycle, createProcessRunner } from './process-lifecycle.mjs';

test('helper container bounds stdin and preserves create/start interactive lifecycle', async () => {
  const commands = [];
  const containers = new Set();
  const docker = async args => {
    commands.push(args);
    if (args[0] === 'create') return 'abcdef123456';
    return '0';
  };
  const capture = async (_command, args, options) => {
    commands.push(args);
    assert.equal(options.input, 'sql');
    assert.equal(options.allowEarlyStdinClose, true);
    return { output: 'ok\n' };
  };
  const lifecycle = createContainerLifecycle({ docker, capture, containers, assert, runId: 'unit', maxInput: 16 });
  assert.equal(await lifecycle.helperContainer(['image', 'sql'], 100, 'sql', { allowEarlyStdinClose: true }), 'ok');
  assert.deepEqual([...containers], ['abcdef123456']);
  assert(commands.some(args => args.includes('--interactive')));
  await assert.rejects(lifecycle.helperContainer([], 100, 'x'.repeat(17)), /helper stdin exceeds fixture bound/);
});

test('process runner bounds accumulated output and removes closed children', async () => {
  const child = new EventEmitter();
  child.stdout = new EventEmitter();
  child.stderr = new EventEmitter();
  child.stdin = new EventEmitter();
  child.stdin.end = () => {};
  child.kill = () => {};
  const children = new Set();
  const runner = createProcessRunner({ spawn: () => child, cwd: '/', children, maxOutput: 4 });
  const pending = runner.capture('fake', []);
  child.stdout.emit('data', Buffer.from('123456'));
  child.emit('close', 0, null);
  const result = await pending;
  assert.equal(result.output, '3456');
  assert.equal(children.size, 0);
});

function fakeChild() {
  const child = new EventEmitter();
  child.stdout = new EventEmitter();
  child.stderr = new EventEmitter();
  child.stdin = new EventEmitter();
  child.stdin.end = () => {};
  child.kill = () => {};
  return child;
}

test('early stdin closure preserves tracking and diagnostics until process close', async () => {
  const child = fakeChild();
  const children = new Set();
  const runner = createProcessRunner({ spawn: () => child, children });
  const pending = runner.capture('probe', [], { allowEarlyStdinClose: true });
  child.stdin.emit('error', Object.assign(new Error('broken pipe'), { code: 'EPIPE' }));
  assert(children.has(child));
  child.stderr.emit('data', Buffer.from('invalid media'));
  child.emit('close', 1, null);
  await assert.rejects(pending, /probe failed \(exit 1\): invalid media/);
  assert.equal(children.size, 0);
});

test('stdin failures remain strict by default and wait for child shutdown', async () => {
  const child = fakeChild();
  const children = new Set();
  const runner = createProcessRunner({ spawn: () => child, children });
  const pending = runner.capture('import', []);
  let killed = false;
  child.kill = () => { killed = true; };
  const error = Object.assign(new Error('broken pipe'), { code: 'EPIPE' });
  child.stdin.emit('error', error);
  assert(killed && children.has(child));
  child.emit('close', null, 'SIGKILL');
  await assert.rejects(pending, error);
  assert.equal(children.size, 0);
});

test('real prefix reader can succeed without consuming all supplied stdin', async () => {
  const children = new Set();
  const runner = createProcessRunner({ children });
  const result = await runner.capture(process.execPath, ['-e',
    'process.stdin.once("data", () => { console.log("metadata"); process.exit(0); });',
  ], { input: Buffer.alloc(2 * 1024 * 1024), allowEarlyStdinClose: true });
  assert.equal(result.output.trim(), 'metadata');
  assert.equal(children.size, 0);
});

test('early stdin closure does not disable the process deadline', async () => {
  const child = fakeChild();
  child.kill = signal => child.emit('close', null, signal);
  const children = new Set();
  const runner = createProcessRunner({ spawn: () => child, children });
  const pending = runner.capture('probe', [], { timeout: 10, allowEarlyStdinClose: true });
  child.stdin.emit('error', Object.assign(new Error('broken pipe'), { code: 'EPIPE' }));
  await assert.rejects(pending, /probe failed \(exit SIGKILL\)/);
  assert.equal(children.size, 0);
});
