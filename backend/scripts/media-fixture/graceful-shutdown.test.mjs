import test from 'node:test';
import assert from 'node:assert/strict';
import { stopInlineAppGracefully } from './graceful-shutdown.mjs';

const received = 'fixture shutdown signal SIGTERM: worker_children=1, workspaces=1';
const cleaned = 'fixture graceful shutdown verified: adapters drained, worker_children=0, workspaces=0';
function fixture({ exit = '0', logs = `${received}\n${cleaned}`, state = {} } = {}) {
  const calls = [];
  const docker = async (args, options) => {
    calls.push({ args, options });
    switch (args[0]) {
      case 'kill': return '';
      case 'wait': return exit;
      case 'inspect': return JSON.stringify({ Running: false, ExitCode: 0, OOMKilled: false, Error: '', ...state });
      case 'logs': return logs;
      default: throw new Error('must not remove or replace container while checking cleanup');
    }
  };
  return { calls, context: { docker, id: 'original-container', assertNoSecret: () => {} } };
}

test('native inline shutdown requires actual SIGTERM, normal exit and original-process cleanup before removal', async () => {
  const { calls, context } = fixture();
  assert.deepEqual(await stopInlineAppGracefully(context), { heldChildren: 1, heldWorkspaces: 1 });
  assert.deepEqual(calls.map(call => call.args[0]), ['kill', 'wait', 'inspect', 'logs']);
  assert.deepEqual(calls[0].args, ['kill', '--signal', 'TERM', 'original-container']);
  assert.equal(calls[1].options.timeout, 30000);
  assert(calls.every(call => call.args.at(-1) === 'original-container'));
});

test('new empty tmpfs, elapsed worker timeout, missing drain proof and reversed evidence do not prove graceful cleanup', async () => {
  for (const logs of [cleaned, `${received}\nnew container workspace empty`, `${received.replace('worker_children=1', 'worker_children=0')}\n${cleaned}`, `${received.replace('workspaces=1', 'workspaces=0')}\n${cleaned}`, `${cleaned}\n${received}`]) {
    await assert.rejects(stopInlineAppGracefully(fixture({ logs }).context));
  }
});

test('forced or failed exit and OOM never qualify as graceful native shutdown', async () => {
  for (const options of [{ exit: '137' }, { state: { ExitCode: 1 } }, { state: { OOMKilled: true } }, { state: { Running: true } }, { state: { Error: 'failed' } }]) {
    await assert.rejects(stopInlineAppGracefully(fixture(options).context));
  }
});
