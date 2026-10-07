import { lstat, readdir, rm, statfs } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

export const MAX_TARGET_BYTES = 16 * 1024 ** 3;
export const MIN_FREE_BYTES = 20 * 1024 ** 3;
const artifactNames = new Set(['debug', 'release', 'tmp', 'turso-probe', 'turso-probe-call-args', '.rustc_info.json', 'CACHEDIR.TAG']);

async function targetExists(target) {
  try {
    const info = await lstat(target);
    if (!info.isDirectory() || info.isSymbolicLink()) throw new Error(`Refusing non-directory or symlink target: ${target}`);
    return true;
  } catch (error) {
    if (error.code === 'ENOENT') return false;
    throw error;
  }
}

export function checkBudget(targetBytes, freeBytes) {
  if (targetBytes > MAX_TARGET_BYTES || freeBytes < MIN_FREE_BYTES) {
    throw new Error(`Build budget exceeded: target ${(targetBytes / 1024 ** 3).toFixed(1)} GiB, free ${(freeBytes / 1024 ** 3).toFixed(1)} GiB. Limit is 16 GiB of artifacts and at least 20 GiB free. Stop builds, inspect disk use, then run just clean-builds in inactive worktrees. Reports are preserved.`);
  }
}

export async function checkSpace(workspace) {
  const target = path.join(workspace, 'target');
  let bytes = 0;
  if (await targetExists(target)) {
    const result = spawnSync('du', ['-s', '-B1', '--', target], { encoding: 'utf8', timeout: 30000 });
    if (result.error || result.status !== 0) throw new Error(`Cannot measure ${target}: ${result.error?.message ?? result.stderr}`);
    bytes = Number(result.stdout.split(/\s+/)[0]);
    if (!Number.isSafeInteger(bytes) || bytes < 0) throw new Error(`Invalid disk usage for ${target}`);
  }
  const fs = await statfs(workspace, { bigint: true });
  checkBudget(bytes, Number(fs.bavail * fs.bsize));
  console.log(`Build resources: target ${(bytes / 1024 ** 3).toFixed(2)} GiB, free ${(Number(fs.bavail * fs.bsize) / 1024 ** 3).toFixed(1)} GiB.`);
}

export async function cleanArtifacts(target) {
  if (!await targetExists(target)) return;
  const entries = await readdir(target);
  const unknown = entries.filter(name => !['media-reports', '.future-incompat-report.json'].includes(name) && !artifactNames.has(name));
  if (unknown.length) throw new Error(`Refusing cleanup with unknown target entries: ${unknown.join(', ')}. Inspect them first.`);
  for (const name of entries.filter(name => artifactNames.has(name))) {
    await rm(path.join(target, name), { recursive: true, force: true });
  }
  console.log(`Removed build artifacts from ${target}; media-reports preserved.`);
}

export async function cleanWorkspace(workspace, scope = '') {
  if (scope && (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(scope) || scope === 'media-reports' || artifactNames.has(scope))) {
    throw new Error('Cleanup target must be one named Cargo target directory under target/, not a path or report directory.');
  }
  const target = path.join(workspace, 'target');
  if (!await targetExists(target)) return;
  await cleanArtifacts(scope ? path.join(target, scope) : target);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const workspace = fileURLToPath(new URL('../', import.meta.url));
  const action = process.argv[2];
  if (!['check', 'clean'].includes(action) || process.argv.length < 3 || process.argv.length > (action === 'clean' ? 4 : 3)) {
    throw new Error('Usage: build-resources.mjs check | clean [TARGET_NAME]');
  }
  if (action === 'check') await checkSpace(workspace);
  else await cleanWorkspace(workspace, process.argv[3] ?? '');
}
