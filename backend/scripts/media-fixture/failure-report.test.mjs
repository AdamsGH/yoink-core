import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, stat } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { boundReport, recentTelegramCalls, writeFailureReport } from './failure-report.mjs';

test('recent Telegram diagnostics retain bounded outcomes despite menu reconciliation', () => {
  const messages = Array.from({ length: 30 }, (_, i) => ({
    method: 'sendmessage', status: 'success', data: { chat_id: -42, message_thread_id: 18, text: `outcome ${i}`, token: 'omit' },
  }));
  const menus = Array.from({ length: 100 }, (_, i) => ({ method: i % 2 ? 'setmycommands' : 'deletemycommands' }));
  const recent = recentTelegramCalls([...messages, ...menus]);
  assert.equal(recent.length, 20);
  assert.equal(recent[0].text, 'outcome 10');
  assert.equal(recent.at(-1).text, 'outcome 29');
  assert.equal(recent.at(-1).thread_id, 18);
  assert.equal(recent.at(-1).chat_id, -42);
  assert(!JSON.stringify(recent).includes('omit'));
  assert.deepEqual(recentTelegramCalls(menus), []);
});

test('bounded reports retain the primary failure and latest diagnostics', () => {
  const report = 'AssertionError: primary failure\n' + 'older diagnostics\n'.repeat(3000) + '\nlatest operation';
  const bounded = boundReport(report, 1024);
  assert.equal(bounded.length, 1024);
  assert(bounded.startsWith('AssertionError: primary failure'));
  assert(bounded.endsWith('latest operation'));
  assert(bounded.includes('diagnostics truncated'));
  assert.equal(boundReport('small'), 'small');
  assert.throws(() => boundReport(report, 1), /invalid report bound/);
});

test('failure reports survive independently of fixture cleanup and never overwrite evidence', async t => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'yoink-report-test-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const directory = path.join(root, 'reports');
  const report = 'Assertion failed\napp=running\nredacted diagnostics\n';
  const destination = await writeFailureReport(directory, '123-fixture', report);
  assert.equal(await readFile(destination, 'utf8'), report);
  if (process.platform !== 'win32') {
    assert.equal((await stat(directory)).mode & 0o777, 0o700);
    assert.equal((await stat(destination)).mode & 0o777, 0o600);
  }
  await assert.rejects(writeFailureReport(directory, '123-fixture', 'replacement'), { code: 'EEXIST' });
  assert.equal(await readFile(destination, 'utf8'), report);
  await assert.rejects(writeFailureReport(directory, '../escape', report), /invalid fixture run ID/);
});
