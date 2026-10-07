import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, readFile, readdir, rm, symlink } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { checkBudget, cleanArtifacts, cleanWorkspace, MAX_TARGET_BYTES, MIN_FREE_BYTES } from './build-resources.mjs';

test('build budgets reject oversize artifacts and low free space', () => {
  assert.doesNotThrow(() => checkBudget(MAX_TARGET_BYTES, MIN_FREE_BYTES));
  assert.throws(() => checkBudget(MAX_TARGET_BYTES + 1, MIN_FREE_BYTES), /Build budget exceeded/);
  assert.throws(() => checkBudget(0, MIN_FREE_BYTES - 1), /Build budget exceeded/);
});

test('named Cargo target cleanup is bounded and preserves reports and siblings', async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'yoink-build-scope-'));
  try {
    const target = path.join(dir, 'target', 'inbox-server');
    await mkdir(path.join(target, 'debug'), { recursive: true });
    await mkdir(path.join(target, 'media-reports'));
    await writeFile(path.join(target, 'media-reports', 'report.log'), 'retained');
    await writeFile(path.join(dir, 'target', 'source.db'), 'untouched');
    for (const invalid of ['../outside', '/tmp', '.', '..', 'nested/target', 'media-reports', 'debug']) {
      await assert.rejects(cleanWorkspace(dir, invalid), /Cleanup target/);
    }
    await writeFile(path.join(target, 'unexpected.db'), 'untouched');
    await assert.rejects(cleanWorkspace(dir, 'inbox-server'), /unknown target entries/);
    assert((await readdir(target)).includes('debug'));
    await rm(path.join(target, 'unexpected.db'));
    await cleanWorkspace(dir, 'inbox-server');
    assert.deepEqual(await readdir(target), ['media-reports']);
    assert.equal(await readFile(path.join(target, 'media-reports', 'report.log'), 'utf8'), 'retained');
    assert.equal(await readFile(path.join(dir, 'target', 'source.db'), 'utf8'), 'untouched');
    await symlink(target, path.join(dir, 'target', 'linked'));
    await assert.rejects(cleanWorkspace(dir, 'linked'), /symlink target/);
    await mkdir(path.join(dir, 'nested'));
    await symlink(path.join(dir, 'target'), path.join(dir, 'nested', 'target'));
    await assert.rejects(cleanWorkspace(path.join(dir, 'nested'), 'inbox-server'), /symlink target/);
    await assert.rejects(cleanWorkspace(dir), /unknown target entries/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('cleanup preserves reports, refuses unknown entries and symlink targets', async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'yoink-build-resources-'));
  try {
    const target = path.join(dir, 'target');
    await mkdir(path.join(target, 'debug'), { recursive: true });
    await mkdir(path.join(target, 'media-reports'));
    await writeFile(path.join(target, 'media-reports', 'report.log'), 'retained evidence');
    await writeFile(path.join(target, 'unexpected.db'), 'not a build artifact');
    await assert.rejects(cleanArtifacts(target), /unknown target entries/);
    assert((await readdir(target)).includes('debug'));
    await rm(path.join(target, 'unexpected.db'));
    await cleanArtifacts(target);
    assert.deepEqual(await readdir(target), ['media-reports']);
    assert.equal(await readFile(path.join(target, 'media-reports', 'report.log'), 'utf8'), 'retained evidence');
    await symlink(target, path.join(dir, 'linked-target'));
    await assert.rejects(cleanArtifacts(path.join(dir, 'linked-target')), /symlink target/);
    await cleanArtifacts(path.join(dir, 'absent'));
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
