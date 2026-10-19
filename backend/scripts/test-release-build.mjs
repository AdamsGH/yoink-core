import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { appendFileSync, closeSync, cpSync, mkdirSync, mkdtempSync, openSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { Releases, builder, execute, handleInterrupts } from './release.mjs';
import { checkSpace } from './build-resources.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const backend = path.join(root, 'backend');
await checkSpace(backend);
const manager = new Releases(root);
const reports = path.join(backend, 'target', 'media-reports', 'release-build');
mkdirSync(reports, { recursive: true });
const image = `yoink/release-build-check:${process.pid}`;
handleInterrupts();

function logged(args, name) {
  const file = path.join(reports, `${name}.log`);
  const fd = openSync(file, 'w', 0o600);
  try {
    const result = spawnSync('docker', args, { cwd: root, stdio: ['ignore', fd, fd], timeout: 1800000 });
    assert(!result.error && result.status === 0, `${name} failed. See ${file}: ${result.error?.message ?? result.status}`);
  } finally { closeSync(fd); }
  return readFileSync(file, 'utf8');
}

function cached(log, stage) {
  const step = log.match(new RegExp(`^#(\\d+) \\[${stage} [^\\]]+\\] (?:RUN|COPY)`, 'm'));
  assert(step, `Missing ${stage} step`);
  return new RegExp(`^#${step[1]} CACHED$`, 'm').test(log);
}

manager.locked(() => {
  const context = mkdtempSync(path.join(backend, 'target', 'release-build-'));
  let builderReady = false;
  try {
    for (const name of ['Dockerfile', '.dockerignore', 'Cargo.toml', 'Cargo.lock', 'crates', 'worker']) {
      cpSync(path.join(backend, name), path.join(context, name), { recursive: true, preserveTimestamps: true });
    }
    manager.ensureBuilder();
    builderReady = true;
    const build = (name, ver) => logged(['buildx', 'build', '--builder', builder, '--load', '--progress=plain', '--target', 'runtime', '--build-arg', `YOINK_RELEASE_VERSION=${ver}`, '--label', `org.opencontainers.image.version=${ver}`, '--tag', image, context], name);
    build('baseline', '0.1.0');
    assert(cached(build('unchanged', '0.1.0'), 'rust-build'), 'Unchanged builds must reuse Rust output');
    assert(cached(build('version-only', '0.1.1'), 'rust-build'), 'Release metadata must not invalidate Rust compilation');
    writeFileSync(path.join(context, 'worker', 'release-build-probe.txt'), `worker-only fixture ${process.pid}\n`);
    assert(cached(build('worker-only', '0.1.1'), 'rust-build'), 'Worker changes must not rebuild Rust');
    appendFileSync(path.join(context, 'crates', 'yoink-worker-launcher', 'src', 'main.rs'), `\n// Release build fixture ${process.pid}.\n`);
    const rustChange = build('rust-change', '0.1.1');
    assert(!cached(rustChange, 'rust-build'), 'Rust changes must compile affected inputs');
    assert(rustChange.includes('Compiling yoink-worker-launcher'), 'Changed launcher must recompile');
    assert(!rustChange.includes('Compiling yoink-server'), 'Launcher-only changes must retain the cached server');
    logged(['run', '--rm', '--pull', 'never', '--network', 'none', '--memory', '512m', '--cpus', '1', '--pids-limit', '128', '--env', 'YOINK_TEST_RELEASE_VERSION=0.1.1', '--entrypoint', 'python', image, '-B', '-m', 'unittest', 'discover', '-s', '/opt/yoink-worker', '-p', 'test_*.py'], 'worker-tests');
    console.log(`Release build checks passed: unchanged, version-only, worker-only, focused Rust rebuild and real worker tests. Logs: ${reports}`);
  } finally {
    rmSync(context, { recursive: true, force: true });
    try {
      const existing = execute(['image', 'inspect', image, '--format', '{{.Id}}'], { optional: true });
      if (existing) execute(['image', 'rm', image]);
    } finally {
      // Keep the bounded cache that subsequent production builds are meant to reuse.
      if (builderReady) manager.finishBuilder();
    }
  }
});
