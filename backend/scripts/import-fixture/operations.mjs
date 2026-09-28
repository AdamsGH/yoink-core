import assert from 'node:assert/strict';
import { readFile, stat } from 'node:fs/promises';
import path from 'node:path';
import { run, assertNoSecrets } from './process.mjs';
import { compareAllContent } from './content.mjs';
import { nativeRequest } from './native.mjs';

export async function operationsChecks({ backend, root, database, query, contract, native, secrets, image }) {
  const binary = path.join(backend, 'target/debug/yoink-ops');
  for (const container of image ? [false, true] : [false]) {
    const label = container ? 'container' : 'host';
    const backup = path.join(root, `${label} backup ' ; --.ybk`);
    const restored = path.join(root, `${label}-restored.db`);
    const mapped = file => {
      const relative = path.relative(root, file);
      assert(relative && !relative.startsWith('..') && !path.isAbsolute(relative));
      return `/fixture/${relative}`;
    };
    const invoke = async (action, source, destination) => {
      const argv = [action, '--source', container ? mapped(source) : source, '--destination', container ? mapped(destination) : destination];
      const args = container ? ['run', '--rm', '--pull', 'never', '--network', 'none', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', '--user', `${process.getuid()}:${process.getgid()}`, '--tmpfs', '/tmp:rw,nosuid,nodev,noexec,mode=1777', '-e', 'TMPDIR=/tmp', '-e', 'TURSO_TMPDIR=/tmp', '--mount', `type=bind,source=${root},target=/fixture`, image, ...argv] : argv;
      const result = await run(container ? 'docker' : binary, args, { cwd: backend, timeout: 120_000, env: { PATH: process.env.PATH, HOME: process.env.HOME } });
      assertNoSecrets(result.stdout + result.stderr, secrets);
      return JSON.parse(result.stdout);
    };
    const before = await invoke('backup', database, backup);
    assert.equal(before.operation, 'backup');
    assert(before.bytes > 0 && /^[a-f0-9]{64}$/.test(before.sha256));
    assert.equal((await stat(backup)).mode & 0o777, 0o600);
    const after = await invoke('restore', backup, restored);
    assert.equal(after.operation, 'restore');
    assert.equal(after.bytes, before.bytes);
    assert.equal(after.sha256, before.sha256);
    assert.equal((await stat(restored)).mode & 0o777, 0o600);
    await compareAllContent(query, contract, native, restored);
    const ledger = await nativeRequest(native, restored, 'SELECT COUNT(*) FROM runtime_migrations');
    assert.equal(Number(ledger.rows[0][0]), 4);
    const sequence = await nativeRequest(native, restored, "INSERT INTO api_keys(name,key_hash,prefix,scopes,created_by,created_at) VALUES ('restore-sequence','restore-sequence-hash','p','[]',9007199254740993,0) RETURNING CAST(id AS TEXT)");
    assert.equal(sequence.rows[0][0], '2000000001', 'restore preserves native allocator high-water');
    const sentinel = await readFile(backup);
    for (const [action, source, destination] of [['backup', database, backup], ['restore', backup, database]]) {
      await assert.rejects(invoke(action, source, destination), error => {
        assert.equal(error.category, 'exit');
        assertNoSecrets(`${error.stdout ?? ''}${error.stderr ?? ''}${error.message}`, secrets);
        return true;
      });
    }
    assert.deepEqual(await readFile(backup), sentinel);
    await compareAllContent(query, contract, native, database);
    if (container) {
      for (const [action, source, diagnostic] of [['backup', database, 'native snapshot failed'], ['restore', backup, 'backup/restore storage operation failed']]) {
        const result = await run('docker', ['run', '--rm', '--pull', 'never', '--network', 'none', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', '--user', `${process.getuid()}:${process.getgid()}`, '--tmpfs', '/tmp:rw,nosuid,nodev,noexec,mode=1777', '--tmpfs', `/small:rw,nosuid,nodev,noexec,size=64k,mode=0700,uid=${process.getuid()},gid=${process.getgid()}`, '-e', 'TMPDIR=/tmp', '-e', 'TURSO_TMPDIR=/tmp', '--mount', `type=bind,source=${root},target=/fixture`, '--entrypoint', '/bin/sh', image, '-c', 'status=0; /usr/local/bin/yoink-ops "$1" --source "$2" --destination /small/out || status=$?; test "$status" != 0 && test ! -e /small/out && test ! -e /small/out.yoink-lock && test -z "$(ls -A /small)"', 'fixture', action, mapped(source)], { cwd: backend, timeout: 120_000, env: { PATH: process.env.PATH, HOME: process.env.HOME } });
        assertNoSecrets(result.stdout + result.stderr, secrets);
        assert(result.stderr.includes(diagnostic), `${action} ENOSPC must fail explicitly and clean only owned staging`);
      }
      await compareAllContent(query, contract, native, database);
      console.log('Backup/restore ENOSPC: no publication, owned staging cleaned, source content unchanged.');
    }
    console.log(`Backup/restore ${label}: all 41 tables and allocator state verified, collision safety preserved.`);
  }
}
