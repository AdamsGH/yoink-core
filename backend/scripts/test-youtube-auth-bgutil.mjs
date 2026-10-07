import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdirSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const name = `yoink-bgutil-check-${randomUUID().slice(0, 12)}`;
const image = 'brainicism/bgutil-ytdlp-pot-provider:2.0.0@sha256:ed86b6fdd5e430ddd7c8ce1adb55e1ab54db7c7dbc1bcbf3a82454a85b971164';
const directory = fileURLToPath(new URL(`../media-reports/${name}/`, import.meta.url));
function docker(args, input) {
  const result = spawnSync('docker', args, { input, encoding: 'utf8', timeout: 60000, maxBuffer: 1048576 });
  if (result.error || result.status !== 0) {
    mkdirSync(directory, { recursive: true, mode: 0o700 });
    const report = `${directory}${args[0]}-${randomUUID()}.log`;
    writeFileSync(report, `${result.error?.message ?? ''}\n${result.stdout ?? ''}\n${result.stderr ?? ''}`, { mode: 0o600 });
    throw new Error(`docker ${args[0]} failed, status ${result.status}. Restricted diagnostics: ${report}`);
  }
  return result.stdout;
}
docker(['image', 'inspect', image]);
try {
  docker(['run', '-d', '--pull', 'never', '--name', name, '--init', '--network', 'none', '--user', '1000:1000', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', '--memory', '512m', '--cpus', '1', '--pids-limit', '64', '--tmpfs', '/tmp:size=64m,uid=1000,gid=1000,mode=0700,noexec,nosuid,nodev', image]);
  const probe = docker(['exec', '-i', name, 'node', '--input-type=module'], `
    import assert from 'node:assert/strict';
    assert.equal(process.getuid(), 1000);
    let response;
    for (let attempt = 0; attempt < 100; attempt++) {
      try { response = await fetch('http://127.0.0.1:4416/ping'); if (response.ok) break; } catch {}
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    assert.ok(response?.ok, 'bgutil ping unavailable');
    const body = await response.json();
    assert.equal(body.version, '2.0.0');
    console.log('bgutil 2.0.0 ping passed under UID1000, read-only root and no network.');
  `);
  console.log(probe.trim());
} catch (error) {
  const logs = spawnSync('docker', ['logs', name], { encoding: 'utf8', timeout: 10000, maxBuffer: 1048576 });
  mkdirSync(directory, { recursive: true, mode: 0o700 });
  writeFileSync(`${directory}startup.log`, `${logs.stdout ?? ''}\n${logs.stderr ?? ''}`, { mode: 0o600 });
  throw error;
} finally {
  const filter = `name=^/${name}$`;
  if (docker(['ps', '-a', '-q', '--filter', filter]).trim()) docker(['rm', '-f', name]);
  assert.equal(docker(['ps', '-a', '-q', '--filter', filter]).trim(), '');
}
