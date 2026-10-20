import assert from 'node:assert/strict';
import { cpSync, existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync, appendFileSync } from 'node:fs';
import { spawn } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { Releases, bump, newer, version, inputHash, snapshot, repository, legacyCacheOrder } from './release.mjs';

test('bootstrap failure attempts bounded cleanup of the owned builder', () => {
  const calls = [];
  const manager = new Releases('/fixture', (args, options) => {
    calls.push([args, options]);
    if (args.includes('--bootstrap')) throw new Error('bootstrap failure');
    return 'Driver: docker-container';
  });
  assert.throws(() => manager.ensureBuilder(), /bootstrap failure/);
  const [stop, options] = calls.at(-1);
  assert.deepEqual(stop, ['buildx', 'stop', 'yoink-core-release']);
  assert.equal(options.timeout, 30000);
});

test('YouTube builds reuse identical immutable tags and reject changed inputs', t => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'youtube-release-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const source = fileURLToPath(new URL('../../', import.meta.url));
  writeFileSync(path.join(root, '.env'), 'yoink_youtube_image=yoink/youtube-services:0.2.0\n');
  for (const file of ['backend/Dockerfile.youtube-auth-gateway', 'backend/worker/youtube_auth_adapter.mjs', 'backend/scripts/youtube-auth-gateway-limits.mjs', 'docker/youtubei-service/package.json', 'docker/youtubei-service/package-lock.json', 'docker/youtubei-service/index.js', 'docker/youtubei-service/lifecycle.mjs', 'docker/youtube-bundle/supervisor.mjs', 'docker/youtube-bundle/health.mjs']) {
    mkdirSync(path.dirname(path.join(root, file)), { recursive: true });
    cpSync(path.join(source, file), path.join(root, file));
  }
  let image = null;
  let builds = 0;
  let mutateDuringBootstrap = false;
  const manager = new Releases(root, args => {
    if (args[0] === 'image') return image && JSON.stringify(image);
    if (args[1] === 'inspect') {
      if (mutateDuringBootstrap && args.includes('--bootstrap')) appendFileSync(path.join(root, 'docker/youtube-bundle/health.mjs'), '\n');
      return 'Driver: docker-container';
    }
    if (args[1] === 'build') {
      assert.equal(args[args.indexOf('--tag') + 1], 'yoink/youtube-services:0.2.0');
      builds++;
      const label = args[args.indexOf('--label') + 1];
      const split = label.indexOf('=');
      image = { Config: { Labels: { [label.slice(0, split)]: label.slice(split + 1) } } };
    }
    return '';
  });
  manager.tool('youtube');
  manager.tool('youtube');
  assert.equal(builds, 1);
  appendFileSync(path.join(root, 'docker/youtube-bundle/health.mjs'), '\n');
  assert.throws(() => manager.tool('youtube'), /immutable/);
  assert.equal(builds, 1);
  image = null;
  mutateDuringBootstrap = true;
  assert.throws(() => manager.tool('youtube'), /inputs changed/);
  assert.equal(builds, 1);
  assert(!readdirSync(path.join(root, 'backend/target')).some(name => name.startsWith('youtube-context-')));
});

function fixture(t, base = os.tmpdir()) {
  const root = mkdtempSync(path.join(base, 'yoink-release-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const backend = path.join(root, 'backend');
  mkdirSync(path.join(backend, 'crates'), { recursive: true });
  mkdirSync(path.join(backend, 'worker'));
  for (const name of ['Dockerfile', '.dockerignore', 'Cargo.toml', 'Cargo.lock']) writeFileSync(path.join(backend, name), name);
  writeFileSync(path.join(backend, 'VERSION'), '0.1.0\n');
  writeFileSync(path.join(root, '.env'), 'secret=untouched\nyoink_rust_image=old\n', { mode: 0o600 });
  let sequence = 0;
  const images = new Map();
  const add = ref => {
    const image = { Id: `sha256:${(++sequence).toString(16).padStart(64, '0')}`, Config: { Labels: {} } };
    images.set(ref, image); return image;
  };
  const old = add('old');
  const previous = add('previous');
  let running = old.Id;
  const calls = [];
  const fail = new Set();
  const inUse = new Set();
  let healthy = true;
  const run = (args, { optional, env } = {}) => {
    calls.push(args);
    if (args[0] === 'image' && args[1] === 'inspect') {
      const image = images.get(args[2]);
      if (!image && !optional) throw new Error('image missing');
      return image ? JSON.stringify(image) : null;
    }
    if (args[0] === 'image' && args[1] === 'tag') { images.set(args[3], images.get(args[2])); return ''; }
    if (args[0] === 'image' && args[1] === 'rm') { images.delete(args[2]); return ''; }
    if (args[0] === 'image' && args[1] === 'ls') {
      return [...images].filter(([ref]) => ref.includes(':')).map(([ref, image]) => {
        const [Repository, Tag] = ref.split(':');
        return JSON.stringify({ Repository, Tag, ID: image.Id });
      }).join('\n');
    }
    if (args[0] === 'inspect' && args[1] === '--format') return [...inUse].join('\n');
    if (args[0] === 'inspect') return JSON.stringify({ image: running, state: { Running: true, Health: { Status: healthy ? 'healthy' : 'unhealthy' } } });
    if (args[0] === 'ps') return inUse.size ? 'foreign-container' : '';
    if (args[0] === 'compose') {
      if (args[1] === 'up') {
        if (fail.has('recreate-before-stop')) throw new Error('stale container inventory');
        const ref = env?.yoink_rust_image ?? process.env.yoink_rust_image ?? readFileSync(path.join(root, '.env'), 'utf8').match(/^yoink_rust_image=(.*)$/m)[1];
        running = images.get(ref).Id;
        if (fail.has(ref)) throw new Error('startup failed');
      }
      return '';
    }
    if (args[0] === 'buildx') {
      if (args[1] === 'inspect') return 'Driver: docker-container';
      if (args[1] === 'build') {
        if (fail.has('build')) throw new Error('compile failed');
        if (!args.includes('--tag')) return '';
        const image = add(args[args.indexOf('--tag') + 1]);
        for (let i = 0; i < args.length; i++) if (args[i] === '--label') {
          const [key, value] = args[i + 1].split('='); image.Config.Labels[key] = value;
        }
      }
      return '';
    }
    throw new Error(`Unexpected command ${args.join(' ')}`);
  };
  const manager = new Releases(root, run);
  manager.locked(() => manager.initialize('old', 'previous', '0.1.0-legacy.1'));
  const next = () => { writeFileSync(path.join(backend, 'VERSION'), '0.1.1\n'); return manager.build(); };
  return { root, backend, manager, calls, images, add, fail, inUse, old, previous, next, setHealthy: value => { healthy = value; } };
}

test('SemVer validation, precedence and explicit bumping', () => {
  for (const bad of ['1.02.3', '../1.2.3', '1.2', 'v1.2.3', '1.2.3+build', '1.2.3-01']) assert.throws(() => version(bad));
  assert.equal(bump('0.9.9', 'minor'), '0.10.0');
  assert.equal(bump('0.9.9', 'major'), '1.0.0');
  assert.equal(bump('0.9.9', 'patch'), '0.9.10');
  assert.throws(() => bump('0.1.0', 'auto'));
  const ordered = ['0.1.0-alpha', '0.1.0-alpha.1', '0.1.0-alpha.2', '0.1.0-alpha.11', '0.1.0-beta', '0.1.0', '0.1.1', '1.0.0'];
  for (let i = 1; i < ordered.length; i++) {
    assert(newer(ordered[i], ordered[i - 1]));
    assert(!newer(ordered[i - 1], ordered[i]));
    assert(!newer(ordered[i], ordered[i]));
  }
});

test('legacy cache cleanup selects proven Yoink records and descendants, never shared parents', () => {
  const records = [
    { ID: 'base', Description: 'shared Rust base', Reclaimable: true },
    { ID: 'ours', Description: 'cargo build -p yoink-server', Parents: ['base'], Reclaimable: true },
    { ID: 'child', Description: 'WORKDIR', Parents: ['ours'], Reclaimable: true },
    { ID: 'foreign', Description: 'another project', Parents: ['base'], Reclaimable: true },
    { ID: 'shared-mount', Type: 'exec.cachemount', Description: 'cargo build -p yoink-server', Parents: ['ours'], Reclaimable: true },
  ];
  assert.deepEqual(legacyCacheOrder(records), ['child', 'ours']);
  records[1].Reclaimable = false;
  assert.throws(() => legacyCacheOrder(records), /Stop active/);
});

test('adoption changes only the image pin and retains rollback without restarting', t => {
  const f = fixture(t);
  assert.equal(readFileSync(path.join(f.root, '.env'), 'utf8'), `secret=untouched\nyoink_rust_image=${repository}:0.1.0\n`);
  assert.equal(f.manager.read().previous.version, '0.1.0-legacy.1');
  assert.equal(f.manager.read().confirmed, false);
  assert(!f.calls.some(a => a[0] === 'compose'));
});

test('lock rejects concurrent releases and releases after failure', t => {
  const { manager } = fixture(t);
  assert.throws(() => manager.locked(() => manager.locked(() => {})), /locked/);
  manager.locked(() => {});
});

test('build fingerprints ignore docs but cover worker and source changes', t => {
  const { backend } = fixture(t);
  const initial = inputHash(backend);
  writeFileSync(path.join(backend, 'README.md'), 'prose');
  assert.equal(inputHash(backend), initial);
  writeFileSync(path.join(backend, 'worker', 'engine.py'), 'source');
  assert.notEqual(inputHash(backend), initial);
});

test('release context is a verified immutable snapshot, not the mutable working tree', t => {
  const { backend } = fixture(t);
  const hash = inputHash(backend);
  const context = snapshot(backend, hash);
  writeFileSync(path.join(backend, 'worker', 'engine.py'), 'new edits');
  assert.equal(inputHash(context), hash);
  assert.notEqual(inputHash(backend), hash);
  assert.throws(() => snapshot(backend, hash), /inputs changed/);
});

test('immutable builds reuse the same version and require a bump for changed inputs', t => {
  const f = fixture(t);
  assert.throws(() => f.manager.build(), /immutable/);
  f.manager.confirm();
  const state = f.next();
  assert.equal(state.candidate.version, '0.1.1');
  f.manager.build();
  assert.equal(f.calls.filter(a => a[0] === 'buildx' && a[1] === 'build').length, 1);
  writeFileSync(path.join(f.backend, 'worker', 'engine.py'), 'changed');
  assert.throws(() => f.manager.build(), /immutable/);
});

test('healthy deployment still requires confirmation and retains exactly one rollback', t => {
  const f = fixture(t);
  f.manager.confirm(); f.next();
  const state = f.manager.deploy();
  assert.equal(state.current.version, '0.1.1');
  assert.equal(state.previous.version, '0.1.0');
  assert.equal(state.confirmed, false);
  writeFileSync(path.join(f.backend, 'VERSION'), '0.1.2\n');
  assert.equal(f.manager.build().candidate.version, '0.1.2');
  f.manager.confirm();
  assert.equal(f.manager.read().previous, null);
  assert(!f.images.has(`${repository}:0.1.0`));
  assert(f.images.has(`${repository}:0.1.1`));
});

test('unconfirmed releases accept a newer candidate without accepting or deleting rollback', t => {
  const f = fixture(t);
  const state = f.next();
  assert.equal(state.confirmed, false);
  assert.equal(state.current.version, '0.1.0');
  assert.equal(state.previous.version, '0.1.0-legacy.1');
  assert.equal(state.candidate.version, '0.1.1');
  assert.equal(f.manager.read().candidate.version, '0.1.1');
  assert(!f.calls.some(a => a[0] === 'compose' || (a[0] === 'image' && a[1] === 'rm')));
  writeFileSync(path.join(f.backend, 'VERSION'), '0.1.2\n');
  assert.throws(() => f.manager.build(), /existing candidate/);
  f.manager.discard();
  assert.equal(f.manager.read().previous.version, '0.1.0-legacy.1');
  writeFileSync(path.join(f.backend, 'VERSION'), '0.1.1\n');
  assert.throws(() => f.manager.build(), /Discarded versions/);
});

test('replacement retains the rollback anchor across successive unconfirmed fixes', t => {
  const f = fixture(t);
  f.next();
  let state = f.manager.deploy();
  assert.equal(state.current.version, '0.1.1');
  assert.equal(state.previous.version, '0.1.0-legacy.1');
  assert.equal(state.confirmed, false);
  assert(!f.images.has(`${repository}:0.1.0`));
  assert(f.images.has(`${repository}:0.1.0-legacy.1`));
  writeFileSync(path.join(f.backend, 'VERSION'), '0.1.2\n');
  f.manager.build();
  state = f.manager.deploy();
  assert.equal(state.current.version, '0.1.2');
  assert.equal(state.previous.version, '0.1.0-legacy.1');
  assert(!f.images.has(`${repository}:0.1.1`));
  assert.equal(f.manager.rollback().current.version, '0.1.0-legacy.1');
});

test('failed replacement restores the immediate runtime without losing the older anchor', t => {
  const f = fixture(t);
  f.next();
  f.fail.add(`${repository}:0.1.1`);
  assert.throws(() => f.manager.deploy(), /Previous release restored/);
  const state = f.manager.read();
  assert.equal(state.current.version, '0.1.0');
  assert.equal(state.previous.version, '0.1.0-legacy.1');
  assert.equal(state.candidate.version, '0.1.1');
  assert.equal(state.confirmed, false);
  assert(f.images.has(`${repository}:0.1.0`));
  assert.match(readFileSync(path.join(f.root, '.env'), 'utf8'), /yoink_rust_image=yoink\/rust-backend:0.1.0\n/);
});

test('failed recreation restores the pin without recreating the unchanged healthy runtime', t => {
  const f = fixture(t);
  f.next();
  f.fail.add('recreate-before-stop');
  assert.throws(() => f.manager.deploy(), /Previous release restored/);
  const state = f.manager.read();
  assert.equal(state.current.version, '0.1.0');
  assert.equal(state.previous.version, '0.1.0-legacy.1');
  assert.equal(state.candidate.version, '0.1.1');
  assert.equal(state.phase, undefined);
  assert.equal(f.calls.filter(a => a[0] === 'compose' && a[1] === 'up').length, 1);
  assert.match(readFileSync(path.join(f.root, '.env'), 'utf8'), /yoink_rust_image=yoink\/rust-backend:0.1.0\n/);
});

test('explicit recovery skips Compose when the superseded runtime never stopped', t => {
  const f = fixture(t);
  f.next();
  const state = f.manager.read();
  state.superseded = state.current;
  state.current = state.candidate;
  delete state.candidate;
  state.phase = 'deploying';
  f.manager.write(state);
  f.fail.add('recreate-before-stop');
  const recovered = f.manager.rollback();
  assert.equal(recovered.current.version, '0.1.0');
  assert.equal(recovered.previous.version, '0.1.0-legacy.1');
  assert.equal(recovered.candidate.version, '0.1.1');
  assert.equal(recovered.phase, undefined);
  assert(!f.calls.some(a => a[0] === 'compose'));
});

test('failed replacement recovery persists all images and rollback restores the pre-deploy state', t => {
  const f = fixture(t);
  f.next();
  f.fail.add(`${repository}:0.1.1`); f.fail.add(`${repository}:0.1.0`);
  assert.throws(() => f.manager.deploy(), /Automatic rollback failed/);
  let state = f.manager.read();
  assert.equal(state.phase, 'deploying');
  assert.equal(state.superseded.version, '0.1.0');
  assert.equal(state.previous.version, '0.1.0-legacy.1');
  assert.throws(() => f.manager.confirm(), /Recover/);
  f.fail.clear();
  state = f.manager.rollback();
  assert.equal(state.current.version, '0.1.0');
  assert.equal(state.previous.version, '0.1.0-legacy.1');
  assert.equal(state.candidate.version, '0.1.1');
  assert.equal(state.phase, undefined);
  assert.equal(state.superseded, undefined);
  assert.equal(f.manager.deploy().current.version, '0.1.1');
});

test('retirement refuses in-use images and can resume or roll back after restart', t => {
  for (const recover of ['deploy', 'rollback']) {
    const f = fixture(t);
    f.next();
    f.inUse.add(f.old.Id);
    assert.throws(() => f.manager.deploy(), /referenced/);
    assert.equal(f.manager.read().phase, 'retiring');
    assert.equal(f.manager.read().superseded.version, '0.1.0');
    assert(f.images.has(`${repository}:0.1.0-legacy.1`));
    assert.throws(() => f.manager.confirm(), /Recover/);
    assert.throws(() => f.manager.cleanImages(), /Recover/);
    const restarted = new Releases(f.root, f.manager.run);
    f.inUse.clear();
    const state = restarted[recover]();
    assert.equal(state.phase, undefined);
    assert.equal(state.superseded, undefined);
    assert.equal(state.previous.version, '0.1.0-legacy.1');
    assert.equal(state.current.version, recover === 'deploy' ? '0.1.1' : '0.1.0');
    assert.equal(state.confirmed, false);
  }
});

test('a crash after retirement deletion can resume cleanup or restore the retained anchor', t => {
  for (const recover of ['deploy', 'rollback']) {
    const f = fixture(t);
    f.next();
    const write = f.manager.write.bind(f.manager);
    f.manager.write = state => {
      if (!state.phase) throw new Error('interrupted state write');
      write(state);
    };
    assert.throws(() => f.manager.deploy(), /interrupted state write/);
    assert.equal(f.manager.read().phase, 'retiring');
    assert(!f.images.has(`${repository}:0.1.0`));
    const restarted = new Releases(f.root, f.manager.run);
    const state = restarted[recover]();
    assert.equal(state.phase, undefined);
    assert.equal(state.current.version, recover === 'deploy' ? '0.1.1' : '0.1.0-legacy.1');
    assert.equal(state.previous.version, recover === 'deploy' ? '0.1.0-legacy.1' : '0.1.1');
    assert.equal(state.confirmed, false);
  }
});

test('retirement rechecks readiness before deleting any superseded image', t => {
  const f = fixture(t);
  f.next();
  f.inUse.add(f.old.Id);
  assert.throws(() => f.manager.deploy(), /referenced/);
  f.inUse.clear();
  f.setHealthy(false);
  assert.throws(() => f.manager.deploy(), /healthy/);
  assert(f.images.has(`${repository}:0.1.0`));
  assert(f.images.has(`${repository}:0.1.0-legacy.1`));
  assert.equal(f.manager.read().phase, 'retiring');
});

test('replacement verifies the retained anchor before startup and before retirement', t => {
  const f = fixture(t);
  f.next();
  f.images.set(`${repository}:0.1.0-legacy.1`, f.old);
  assert.throws(() => f.manager.deploy(), /Immutable/);
  assert(!f.calls.some(a => a[0] === 'compose'));
  assert.equal(f.manager.read().candidate.version, '0.1.1');
  f.images.set(`${repository}:0.1.0-legacy.1`, f.previous);
  f.inUse.add(f.old.Id);
  assert.throws(() => f.manager.deploy(), /referenced/);
  f.inUse.clear();
  f.images.set(`${repository}:0.1.0-legacy.1`, f.old);
  assert.throws(() => f.manager.deploy(), /Immutable/);
  assert(f.images.has(`${repository}:0.1.0`));
  assert.equal(f.manager.read().phase, 'retiring');
});

test('deploy and rollback override stale image exports inherited from root just dotenv', t => {
  const f = fixture(t);
  const original = process.env.yoink_rust_image;
  t.after(() => {
    if (original === undefined) delete process.env.yoink_rust_image;
    else process.env.yoink_rust_image = original;
  });
  process.env.yoink_rust_image = `${repository}:0.1.0`;
  f.manager.confirm(); f.next();
  assert.equal(f.manager.deploy().current.version, '0.1.1');
  process.env.yoink_rust_image = `${repository}:0.1.1`;
  assert.equal(f.manager.rollback().current.version, '0.1.0');
});

test('failed deployment restores old pin and process without losing candidate', t => {
  const f = fixture(t);
  f.manager.confirm(); f.next();
  f.fail.add(`${repository}:0.1.1`);
  assert.throws(() => f.manager.deploy(), /Previous release restored/);
  assert.equal(f.manager.read().current.version, '0.1.0');
  assert.equal(f.manager.read().candidate.version, '0.1.1');
  assert.match(readFileSync(path.join(f.root, '.env'), 'utf8'), /yoink_rust_image=yoink\/rust-backend:0.1.0\n/);
});

test('failed automatic rollback preserves recovery state and explicit rollback repairs it', t => {
  const f = fixture(t);
  f.manager.confirm(); f.next();
  f.fail.add(`${repository}:0.1.1`); f.fail.add(`${repository}:0.1.0`);
  assert.throws(() => f.manager.deploy(), /Automatic rollback failed/);
  assert.equal(f.manager.read().phase, 'deploying');
  assert.throws(() => f.manager.confirm(), /Recover/);
  f.fail.clear();
  f.manager.rollback();
  assert.equal(f.manager.read().current.version, '0.1.0');
  assert.equal(f.manager.read().previous.version, '0.1.1');
  assert.equal(f.manager.read().phase, undefined);
  f.manager.confirm();
  assert.throws(() => f.manager.build(), /Discarded versions cannot be reused/);
});

test('confirmation fails closed for unhealthy runtime, changed tags and in-use rollback', t => {
  const f = fixture(t);
  f.setHealthy(false);
  assert.throws(() => f.manager.confirm(), /healthy/);
  f.setHealthy(true);
  f.inUse.add(f.previous.Id);
  assert.throws(() => f.manager.confirm(), /referenced/);
  f.inUse.clear();
  f.images.set(`${repository}:0.1.0-legacy.1`, f.old);
  assert.throws(() => f.manager.confirm(), /Immutable/);
  assert.equal(f.manager.read().confirmed, false);
});

test('ordinary rollback persists recovery intent before changing the pin or service', t => {
  const f = fixture(t);
  f.manager.confirm(); f.next(); f.manager.deploy();
  f.fail.add(`${repository}:0.1.0`);
  assert.throws(() => f.manager.rollback(), /startup failed/);
  assert.equal(f.manager.read().phase, 'rolling-back');
  assert.equal(f.manager.read().previous.version, '0.1.0');
  assert.throws(() => f.manager.confirm(), /Recover/);
  f.fail.clear();
  assert.equal(f.manager.rollback().current.version, '0.1.0');
  assert.equal(f.manager.read().phase, undefined);
});

test('retention checks exact image configurations, not filesystem ancestry', t => {
  const f = fixture(t);
  f.inUse.add(f.old.Id);
  f.manager.confirm();
  assert(!f.images.has(`${repository}:0.1.0-legacy.1`));
  assert(f.images.has(`${repository}:0.1.0`));
  assert(!f.calls.some(a => a.some(value => value.startsWith('ancestor='))));
});

test('failed builds trim only the dedicated builder and preserve current release', t => {
  const f = fixture(t);
  f.manager.confirm(); f.fail.add('build');
  assert.throws(() => f.next(), /compile failed/);
  assert.equal(f.manager.read().candidate, undefined);
  const prune = f.calls.find(a => a[0] === 'buildx' && a[1] === 'prune');
  assert.equal(prune[prune.indexOf('--builder') + 1], 'yoink-core-release');
  assert(!f.calls.some(a => a[0] === 'system' || (a[0] === 'image' && a[1] === 'prune')));
});

test('tool builds use bounded builder and cannot overwrite production tags', t => {
  const f = fixture(t);
  f.manager.tool('worker');
  const build = f.calls.find(a => a[0] === 'buildx' && a[1] === 'build');
  assert.equal(build[build.indexOf('--target') + 1], 'worker-runtime');
  assert.equal(build[build.indexOf('--tag') + 1], 'yoink/rust-worker:experimental');
  assert(f.calls.some(a => a[0] === 'buildx' && a[1] === 'inspect' && a.includes('--bootstrap')));
  assert(f.calls.some(a => a[0] === 'buildx' && a[1] === 'stop'));
  assert.throws(() => f.manager.tool('runtime'), /Unknown/);
  assert.equal(f.manager.read().current.version, '0.1.0');
});

test('interrupted build registration recovers only matching labelled artifacts', t => {
  const f = fixture(t);
  f.manager.confirm();
  writeFileSync(path.join(f.backend, 'VERSION'), '0.1.1\n');
  const image = f.add(`${repository}:0.1.1`);
  assert.throws(() => f.manager.build(), /Unregistered/);
  image.Config.Labels = { 'org.opencontainers.image.version': '0.1.1', 'io.yoink.input-sha256': inputHash(f.backend) };
  assert.equal(f.manager.build().candidate.id, image.Id);
  assert(!f.calls.some(a => a[0] === 'buildx' && a[1] === 'build'));
});

test('discard removes candidate only and cleanup preserves unrelated repositories', t => {
  const f = fixture(t);
  f.manager.confirm(); f.next(); f.manager.discard();
  assert.throws(() => f.manager.build(), /Discarded versions cannot be reused/);
  f.add(`${repository}:ancient`);
  f.add('yoink/rust-functional-runtime:tldr-v1');
  f.add('unrelated/service:old');
  f.manager.cleanImages();
  assert(!f.images.has(`${repository}:ancient`));
  assert(!f.images.has('yoink/rust-functional-runtime:tldr-v1'));
  assert(f.images.has('unrelated/service:old'));
  assert.equal(f.images.get(`${repository}:experimental`).Id, f.old.Id);
});

test('terminal cancellation removes source snapshot and lock, trims cache and stops builder', async t => {
  const base = fileURLToPath(new URL('../target/', import.meta.url));
  mkdirSync(base, { recursive: true });
  const f = fixture(t, base);
  f.manager.confirm();
  writeFileSync(path.join(f.backend, 'VERSION'), '0.1.1\n');
  const scripts = path.join(f.backend, 'scripts');
  mkdirSync(scripts);
  for (const name of ['release.mjs', 'build-resources.mjs']) cpSync(new URL(name, import.meta.url), path.join(scripts, name));
  const bin = path.join(f.root, 'bin');
  mkdirSync(bin);
  writeFileSync(path.join(bin, 'docker'), `#!${process.execPath}
const fs = require('node:fs');
const path = require('node:path');
const args = process.argv.slice(2);
const root = process.env.RELEASE_FIXTURE;
fs.appendFileSync(path.join(root, 'calls'), JSON.stringify(args) + '\\n');
if (args[0] === 'image' && args[1] === 'inspect') process.exit(1);
if (args[0] === 'buildx' && args[1] === 'inspect') console.log('Driver: docker-container');
if (args[0] === 'buildx' && args[1] === 'build') {
  fs.writeFileSync(path.join(root, 'build-started'), 'ready');
  setInterval(() => {}, 1000);
}
`, { mode: 0o700 });
  const child = spawn(process.execPath, [path.join(scripts, 'release.mjs'), 'build'], {
    cwd: f.root, detached: true, stdio: ['ignore', 'ignore', 'pipe'],
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, RELEASE_FIXTURE: f.root },
  });
  let stderr = '';
  child.stderr.on('data', chunk => { stderr += chunk.toString(); });
  const exit = new Promise(resolve => child.once('exit', (code, signal) => resolve({ code, signal })));
  try {
    for (let i = 0; i < 500 && child.exitCode === null && !existsSync(path.join(f.root, 'build-started')); i++) await delay(10);
    assert(existsSync(path.join(f.root, 'build-started')), `Fake build must reach readiness before interruption: ${stderr}`);
    process.kill(-child.pid, 'SIGINT');
    const result = await Promise.race([exit, delay(5000, undefined, { ref: false }).then(() => { throw new Error('Cancellation did not complete'); })]);
    assert.notEqual(result.code, 0);
    assert(!existsSync(path.join(f.backend, '.release', 'lock')));
    assert(!readdirSync(path.join(f.backend, 'target')).some(name => name.startsWith('release-context-')));
    const calls = readFileSync(path.join(f.root, 'calls'), 'utf8').trim().split('\n').map(JSON.parse);
    assert(calls.some(a => a[0] === 'buildx' && a[1] === 'prune'));
    assert(calls.some(a => a[0] === 'buildx' && a[1] === 'stop'));
  } finally {
    if (child.exitCode === null && child.signalCode === null) process.kill(-child.pid, 'SIGKILL');
    await exit;
  }
});
