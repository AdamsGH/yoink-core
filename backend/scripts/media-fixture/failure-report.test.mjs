import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, stat } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { writeFailureReport } from './failure-report.mjs';

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
