import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { createContainerLifecycle } from './process-lifecycle.mjs';

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
    return { output: 'ok\n' };
  };
  const lifecycle = createContainerLifecycle({ docker, capture, containers, assert, runId: 'unit', maxInput: 16 });
  assert.equal(await lifecycle.helperContainer(['image', 'sql'], 100, 'sql'), 'ok');
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
  const { createProcessRunner } = await import('./process-lifecycle.mjs');
  const runner = createProcessRunner({ spawn: () => child, cwd: '/', children, maxOutput: 4 });
  const pending = runner.capture('fake', []);
  child.stdout.emit('data', Buffer.from('123456'));
  child.emit('close', 0, null);
  const result = await pending;
  assert.equal(result.output, '3456');
  assert.equal(children.size, 0);
});
