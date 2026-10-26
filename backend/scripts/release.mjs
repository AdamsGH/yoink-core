import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { cpSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { checkSpace } from './build-resources.mjs';
import { reportedBuild } from './build-report.mjs';

export const repository = 'yoink/rust-backend';
export const builder = 'yoink-core-release';
const versionPattern = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/;

export function youtubeImage(root) {
  const envFile = path.join(root, '.env');
  const configured = existsSync(envFile) ? readFileSync(envFile, 'utf8').match(/^yoink_youtube_image=(.*)$/m)?.[1]?.trim().replace(/^(['"])(.*)\1$/, '$2') : null;
  const image = configured ?? process.env.yoink_youtube_image ?? 'yoink/youtube-services:0.1.0';
  if (!image.startsWith('yoink/youtube-services:')) throw new Error('Expected yoink/youtube-services:<SemVer>.');
  version(image.slice('yoink/youtube-services:'.length));
  return image;
}

export const youtubeInputs = ['backend/Dockerfile.youtube-auth-gateway', 'backend/worker/youtube_auth_adapter.mjs', 'backend/scripts/youtube-auth-gateway-limits.mjs', 'docker/youtubei-service/package.json', 'docker/youtubei-service/package-lock.json', 'docker/youtubei-service/index.js', 'docker/youtubei-service/lifecycle.mjs', 'docker/youtube-bundle/supervisor.mjs', 'docker/youtube-bundle/health.mjs'];

export function youtubeInputHash(root) {
  const hash = createHash('sha256');
  for (const file of youtubeInputs) {
    const info = lstatSync(path.join(root, file));
    if (!info.isFile()) throw new Error(`Expected a regular YouTube build input: ${file}`);
    hash.update(`${file}\0${info.mode & 0o777}\0`).update(readFileSync(path.join(root, file))).update('\0');
  }
  return hash.digest('hex');
}

export function handleInterrupts() {
  // Let synchronous Docker calls unwind through cleanup after terminal cancellation.
  process.on('SIGINT', () => { process.exitCode = 130; });
  process.on('SIGTERM', () => { process.exitCode = 143; });
}

export function version(value) {
  const match = versionPattern.exec(value);
  if (!match || value.length > 128 || match[4]?.split('.').some(p => /^\d+$/.test(p) && p.length > 1 && p[0] === '0')) {
    throw new Error('Expected SemVer without build metadata, for example 0.1.0.');
  }
  return value;
}

export function bump(value, part) {
  version(value);
  if (!['major', 'minor', 'patch'].includes(part) || value.includes('-')) throw new Error('Bump requires a stable version and major, minor or patch.');
  const numbers = value.split('.').map(BigInt);
  const index = ['major', 'minor', 'patch'].indexOf(part);
  numbers[index]++;
  for (let i = index + 1; i < 3; i++) numbers[i] = 0n;
  return numbers.join('.');
}

export function newer(a, b) {
  version(a); version(b);
  const aa = a.split('-')[0].split('.').map(BigInt);
  const bb = b.split('-')[0].split('.').map(BigInt);
  for (let i = 0; i < 3; i++) if (aa[i] !== bb[i]) return aa[i] > bb[i];
  const ap = a.includes('-') ? a.slice(a.indexOf('-') + 1).split('.') : null;
  const bp = b.includes('-') ? b.slice(b.indexOf('-') + 1).split('.') : null;
  if (!ap || !bp) return Boolean(bp) && !ap;
  for (let i = 0; i < Math.max(ap.length, bp.length); i++) {
    if (ap[i] === undefined || bp[i] === undefined) return bp[i] === undefined;
    if (ap[i] === bp[i]) continue;
    const an = /^\d+$/.test(ap[i]);
    const bn = /^\d+$/.test(bp[i]);
    if (an && bn) return BigInt(ap[i]) > BigInt(bp[i]);
    if (an !== bn) return !an;
    return ap[i] > bp[i];
  }
  return false;
}

export function inputHash(backend) {
  const hash = createHash('sha256');
  const visit = relative => {
    const full = path.join(backend, relative);
    const info = lstatSync(full);
    if (info.isSymbolicLink()) throw new Error(`Build inputs must not be symlinks: ${relative}`);
    if (info.isDirectory()) {
      for (const name of readdirSync(full).sort()) {
        if (!['target', '__pycache__', '.git'].includes(name)) visit(path.join(relative, name));
      }
    } else if (info.isFile() && !relative.endsWith('.pyc')) {
      hash.update(`${relative}\0${info.mode & 0o777}\0`);
      hash.update(readFileSync(full)); hash.update('\0');
    }
  };
  for (const name of ['Dockerfile', '.dockerignore', 'Cargo.toml', 'Cargo.lock', 'crates', 'worker', 'VERSION']) visit(name);
  return hash.digest('hex');
}

export function snapshot(backend, expectedHash) {
  const target = path.join(backend, 'target');
  mkdirSync(target, { recursive: true });
  const context = mkdtempSync(path.join(target, 'release-context-'));
  try {
    for (const name of ['Dockerfile', '.dockerignore', 'Cargo.toml', 'Cargo.lock', 'crates', 'worker', 'VERSION']) {
      cpSync(path.join(backend, name), path.join(context, name), {
        recursive: true, preserveTimestamps: true,
        filter: source => !['target', '__pycache__', '.git'].includes(path.basename(source)) && !source.endsWith('.pyc'),
      });
    }
    if (inputHash(context) !== expectedHash || inputHash(backend) !== expectedHash) throw new Error('Build inputs changed while taking the snapshot. Retry after edits finish.');
    return context;
  } catch (error) {
    rmSync(context, { recursive: true, force: true });
    throw error;
  }
}

export function execute(args, { cwd, live = false, optional = false, env = process.env, timeout = 1800000, reportBase } = {}) {
  const result = reportBase
    ? reportedBuild(args, { cwd, env, timeout, reportBase })
    : spawnSync('docker', args, { cwd, env, encoding: 'utf8', stdio: live ? 'inherit' : 'pipe', timeout, maxBuffer: 16 * 1024 ** 2 });
  if (result.error || result.status !== 0) {
    if (optional) return null;
    throw new Error(`docker ${args.slice(0, 3).join(' ')} failed: ${result.error?.message ?? result.stderr ?? result.status}${reportBase ? `. See ${reportBase}.log` : ''}`);
  }
  return result.stdout?.trim() ?? '';
}

export function legacyCacheOrder(records) {
  // A legacy cache mount can be shared across projects despite its creator description.
  const selected = new Set(records.filter(e => e.Type !== 'exec.cachemount' && /yoink-(?:server|worker-launcher|fixture-server|ops|import)\b|\/opt\/yoink-worker/.test(e.Description)).map(e => e.ID));
  let changed = true;
  while (changed) {
    changed = false;
    for (const entry of records) if (entry.Type !== 'exec.cachemount' && !selected.has(entry.ID) && entry.Parents?.some(id => selected.has(id))) {
      selected.add(entry.ID); changed = true;
    }
  }
  if (records.some(e => selected.has(e.ID) && !e.Reclaimable)) throw new Error('Stop active legacy Yoink builds before pruning their cache.');
  const order = [];
  while (selected.size) {
    const parents = new Set(records.filter(e => selected.has(e.ID)).flatMap(e => e.Parents ?? []));
    const leaves = [...selected].filter(id => !parents.has(id));
    if (!leaves.length) throw new Error('Unexpected cache dependency cycle.');
    for (const id of leaves) { selected.delete(id); order.push(id); }
  }
  return order;
}

export class Releases {
  constructor(root, run = execute) {
    this.root = root;
    this.backend = path.join(root, 'backend');
    this.directory = path.join(this.backend, '.release');
    this.stateFile = path.join(this.directory, 'state.json');
    this.run = (args, options) => run(args, { cwd: root, ...options });
  }

  locked(callback) {
    mkdirSync(this.directory, { recursive: true, mode: 0o700 });
    const lock = path.join(this.directory, 'lock');
    try { mkdirSync(lock); } catch (error) {
      if (error.code !== 'EEXIST') throw error;
      throw new Error(`Release operation already locked at ${lock}. Check for a running release before removing a stale lock.`);
    }
    writeFileSync(path.join(lock, 'pid'), `${process.pid}\n`);
    try { return callback(); } finally { rmSync(lock, { recursive: true }); }
  }

  read() {
    if (!existsSync(this.stateFile)) throw new Error('Initialize release state with release-init first.');
    const state = JSON.parse(readFileSync(this.stateFile, 'utf8'));
    if (state.schema !== 1 || !state.current
      || (state.superseded && (!state.previous || !['deploying', 'retiring', 'rolling-back'].includes(state.phase) || state.candidate))
      || (state.phase === 'retiring' && !state.superseded)) throw new Error('Invalid release state. Do not delete recovery state.');
    for (const entry of [state.current, state.previous, state.candidate, state.superseded].filter(Boolean)) {
      if (entry.image !== `${repository}:${version(entry.version)}` || !/^sha256:[a-f0-9]{64}$/.test(entry.id)) throw new Error('Invalid release image record.');
    }
    state.highestVersion ??= [state.current, state.previous, state.candidate, state.superseded].filter(Boolean).map(e => e.version).reduce((a, b) => newer(a, b) ? a : b);
    version(state.highestVersion);
    return state;
  }

  write(state) {
    writeFileSync(`${this.stateFile}.new`, `${JSON.stringify(state, null, 2)}\n`, { mode: 0o600 });
    renameSync(`${this.stateFile}.new`, this.stateFile);
  }

  image(ref, optional = false) {
    const result = this.run(['image', 'inspect', ref, '--format', '{{json .}}'], { optional });
    return result === null ? null : JSON.parse(result);
  }

  assertImage(entry) {
    if (this.image(entry.image).Id !== entry.id) throw new Error(`Immutable tag changed: ${entry.image}. Restore it before continuing.`);
  }

  pin(image) {
    const file = path.join(this.root, '.env');
    const info = lstatSync(file);
    if (!info.isFile() || info.isSymbolicLink()) throw new Error('Refusing non-regular .env.');
    const source = readFileSync(file, 'utf8');
    if ((source.match(/^yoink_rust_image=/gm) ?? []).length > 1) throw new Error('Duplicate yoink_rust_image in .env.');
    const line = `yoink_rust_image=${image}`;
    const output = /^yoink_rust_image=.*$/m.test(source) ? source.replace(/^yoink_rust_image=.*$/m, line) : `${source.trimEnd()}\n${line}\n`;
    writeFileSync(`${file}.release-new`, output, { mode: info.mode & 0o777 });
    renameSync(`${file}.release-new`, file);
  }

  ready(entry, optional = false) {
    this.assertImage(entry);
    const inspection = this.run(['inspect', 'yoink', '--format', '{"image":{{json .Image}},"state":{{json .State}}}'], { optional });
    if (inspection === null) return false;
    const result = JSON.parse(inspection);
    if (result.image !== entry.id || !result.state.Running || result.state.Health?.Status !== 'healthy') {
      if (optional) return false;
      throw new Error(`Running yoink must be healthy and use ${entry.image}.`);
    }
    return true;
  }

  start(entry) {
    // Compose can fail before stopping the old poller; recovery must not recreate it.
    const alreadyRunning = this.ready(entry, true);
    this.pin(entry.image);
    if (alreadyRunning) return;
    // Root justfile dotenv exports otherwise override the newly written .env pin.
    const env = { ...process.env, yoink_rust_image: entry.image };
    this.run(['compose', 'config', '--quiet'], { env });
    this.run(['compose', 'up', '-d', '--no-deps', '--no-build', '--pull', 'never', '--wait', '--wait-timeout', '360', 'yoink'], { live: true, env });
    this.ready(entry);
  }

  initialize(current, previous, previousVersion) {
    if (existsSync(this.stateFile)) throw new Error('Release state already initialized.');
    const currentVersion = version(readFileSync(path.join(this.backend, 'VERSION'), 'utf8').trim());
    if (previous && (!previousVersion || !newer(currentVersion, previousVersion))) throw new Error('Previous version must precede VERSION.');
    const adopt = (ref, ver) => {
      const id = this.image(ref).Id;
      const image = `${repository}:${version(ver)}`;
      const existing = this.image(image, true);
      if (existing && existing.Id !== id) throw new Error(`Refusing to replace ${image}.`);
      this.run(['image', 'tag', ref, image]);
      return { version: ver, image, id, adopted: true };
    };
    const active = adopt(current, currentVersion);
    this.ready(active);
    const old = previous ? adopt(previous, previousVersion) : null;
    const state = { schema: 1, current: active, previous: old, confirmed: false, highestVersion: currentVersion };
    this.write(state);
    this.pin(active.image);
    return state;
  }

  ensureBuilder() {
    const driver = this.run(['buildx', 'inspect', builder], { optional: true });
    if (driver === null) {
      this.run(['buildx', 'create', '--name', builder, '--driver', 'docker-container', '--driver-opt', 'image=moby/buildkit:v0.33.1,memory=3g,memory-swap=3g,cpu-period=100000,cpu-quota=100000', '--buildkitd-config', path.join(this.backend, 'buildkitd.toml')], { live: true });
    } else {
      if (!/^Driver:\s+docker-container\s*$/m.test(driver)) throw new Error(`Refusing unrelated builder ${builder}.`);
    }
    try {
      this.run(['buildx', 'inspect', builder, '--bootstrap']);
    } catch (error) {
      if (this.run(['buildx', 'stop', builder], { optional: true, timeout: 30000 }) === null) console.error(`Could not stop owned builder ${builder} after bootstrap failed.`);
      throw error;
    }
  }

  trim() {
    this.run(['buildx', 'prune', '--builder', builder, '--all', '--force', '--max-used-space', '8gb', '--min-free-space', '20gb', '--reserved-space', '1gb'], { live: true });
  }

  cacheReport(reportBase, phase) {
    const records = this.run(['buildx', 'du', '--builder', builder, '--format', 'json'], { optional: true });
    if (records === null) return;
    mkdirSync(path.dirname(reportBase), { recursive: true });
    writeFileSync(`${reportBase}.cache-${phase}.jsonl`, `${records}\n`, { mode: 0o600 });
  }

  finishBuilder(reportBase) {
    try {
      this.trim();
      if (reportBase) this.cacheReport(reportBase, 'after');
    }
    finally { this.run(['buildx', 'stop', builder]); }
  }

  tool(name) {
    if (name === 'youtube') {
      const image = youtubeImage(this.root);
      const fingerprint = youtubeInputHash(this.root);
      const existing = this.image(image, true);
      if (existing) {
        if (existing.Config.Labels?.['io.yoink.youtube-input-sha256'] !== fingerprint) throw new Error('YouTube image tag is immutable. Set yoink_youtube_image to a new SemVer.');
        return;
      }
      this.ensureBuilder();
      try {
        const target = path.join(this.backend, 'target');
        mkdirSync(target, { recursive: true });
        const context = mkdtempSync(path.join(target, 'youtube-context-'));
        try {
          for (const file of youtubeInputs) {
            mkdirSync(path.dirname(path.join(context, file)), { recursive: true });
            cpSync(path.join(this.root, file), path.join(context, file), { preserveTimestamps: true });
          }
          if (youtubeInputHash(context) !== fingerprint || youtubeInputHash(this.root) !== fingerprint) throw new Error('YouTube build inputs changed while taking the snapshot.');
          this.run(['buildx', 'build', '--builder', builder, '--load', '--progress=plain', '--file', path.join(context, 'backend/Dockerfile.youtube-auth-gateway'), '--label', `io.yoink.youtube-input-sha256=${fingerprint}`, '--tag', image, context], { live: true });
        } finally { rmSync(context, { recursive: true, force: true }); }
      } finally { this.finishBuilder(); }
      return;
    }
    const tools = {
      worker: ['worker-runtime', 'yoink/rust-worker:experimental'],
      content: ['content-worker', 'yoink/rust-content-worker:experimental'],
      fixture: ['media-fixture', 'yoink/rust-backend-fixture:experimental'],
      importer: ['importer', 'yoink/rust-importer:experimental'],
      operations: ['operations', 'yoink/rust-operations:native'],
      search: ['stats-search-tests', 'yoink/rust-stats-search-tests:experimental'],
      config: ['server-config-tests', null],
      proxy: ['download-proxy-tests', null],
      menus: ['telegram-menu-tests', null],
    };
    if (!Object.hasOwn(tools, name)) throw new Error('Unknown backend tool image.');
    const [target, image] = tools[name];
    this.ensureBuilder();
    try {
      const tags = image ? ['--load', '--tag', image] : ['--output=type=cacheonly'];
      if (name === 'operations') tags.push('--tag', 'yoink/rust-operations:experimental');
      this.run(['buildx', 'build', '--builder', builder, '--progress=plain', '--target', target, ...tags, this.backend], { live: true });
    } finally { this.finishBuilder(); }
  }

  build() {
    if (!existsSync(this.stateFile)) return this.bootstrap();
    const state = this.read();
    if (state.phase === 'bootstrapping') return this.finishBootstrap(state);
    if (state.phase) throw new Error('Interrupted deployment. Run release-rollback before building.');
    const ver = version(readFileSync(path.join(this.backend, 'VERSION'), 'utf8').trim());
    const hash = inputHash(this.backend);
    const existing = [state.current, state.candidate].find(e => e?.version === ver);
    if (existing) {
      this.assertImage(existing);
      if (existing.inputHash !== hash) throw new Error(`Version ${ver} is immutable. Bump VERSION before building changed inputs.`);
      console.log(`Already built: ${existing.image}`);
      return state;
    }
    if (state.candidate) throw new Error('Deploy or discard the existing candidate first.');
    if (!newer(ver, state.highestVersion)) throw new Error(`VERSION must exceed the last built version ${state.highestVersion}. Discarded versions cannot be reused.`);
    state.candidate = this.buildImage(ver, hash);
    state.highestVersion = ver;
    this.write(state);
    return state;
  }

  bootstrap() {
    if (existsSync(this.stateFile)) throw new Error('Release state already initialized.');
    if (this.run(['inspect', 'yoink'], { optional: true }) !== null) {
      throw new Error('Existing yoink container without release state. Adopt it with release-init; bootstrap never replaces a deployment.');
    }
    const ver = version(readFileSync(path.join(this.backend, 'VERSION'), 'utf8').trim());
    const hash = inputHash(this.backend);
    const published = `ghcr.io/adamsgh/yoink-core/backend:${ver}`;
    if (!this.image(`${repository}:${ver}`, true)) {
      const downloaded = this.image(published, true);
      if (downloaded) {
        if (downloaded.Config?.Labels?.['io.yoink.input-sha256'] !== hash || downloaded.Config?.Labels?.['org.opencontainers.image.version'] !== ver) {
          throw new Error('Downloaded runtime does not match this checkout. Use its matching release tag.');
        }
        this.run(['image', 'tag', published, `${repository}:${ver}`]);
      }
    }
    const current = this.buildImage(ver, hash);
    const state = { schema: 1, current, previous: null, confirmed: false, highestVersion: ver, phase: 'bootstrapping' };
    this.write(state);
    return this.finishBootstrap(state);
  }

  finishBootstrap(state) {
    this.assertImage(state.current);
    if (state.current.inputHash !== inputHash(this.backend)) throw new Error('Bootstrap inputs changed. Restore the matching checkout before retrying release-build.');
    this.pin(state.current.image);
    delete state.phase;
    this.write(state);
    return state;
  }

  buildImage(ver, hash) {
    const image = `${repository}:${ver}`;
    const orphan = this.image(image, true);
    if (orphan) {
      if (orphan.Config?.Labels?.['io.yoink.input-sha256'] !== hash || orphan.Config?.Labels?.['org.opencontainers.image.version'] !== ver) {
        throw new Error(`Unregistered immutable image exists: ${image}. Inspect it before continuing.`);
      }
      return { version: ver, image, id: orphan.Id, inputHash: hash };
    }
    this.ensureBuilder();
    let context;
    const reportBase = path.join(this.backend, 'target', 'media-reports', 'releases', ver);
    try {
      this.cacheReport(reportBase, 'before');
      context = snapshot(this.backend, hash);
      this.run(['buildx', 'build', '--builder', builder, '--load', '--progress=plain', '--target', 'runtime', '--build-arg', `YOINK_RELEASE_VERSION=${ver}`, '--label', `org.opencontainers.image.version=${ver}`, '--label', `io.yoink.input-sha256=${hash}`, '--tag', image, context], { reportBase });
      return { version: ver, image, id: this.image(image).Id, inputHash: hash };
    } finally {
      if (context) rmSync(context, { recursive: true, force: true });
      this.finishBuilder(reportBase);
    }
  }

  deploy() {
    const state = this.read();
    if (state.phase === 'bootstrapping') throw new Error('Interrupted initialization. Run release-build before deployment.');
    if (state.phase === 'retiring') return this.finishReplacement(state);
    if (state.phase) throw new Error('Interrupted deployment. Run release-rollback.');
    if (!state.candidate) { this.ready(state.current); this.pin(state.current.image); return state; }
    this.assertImage(state.current);
    if (state.previous) this.assertImage(state.previous);
    const before = structuredClone(state);
    if (state.previous) state.superseded = state.current;
    else state.previous = state.current;
    state.current = state.candidate;
    delete state.candidate;
    state.confirmed = false;
    state.phase = 'deploying';
    this.write(state);
    try {
      this.start(state.current);
      this.run(['image', 'tag', state.current.image, `${repository}:experimental`]);
    } catch (error) {
      try {
        this.start(before.current);
        this.run(['image', 'tag', before.current.image, `${repository}:experimental`]);
        this.write(before);
      }
      catch (recovery) { throw new Error(`${error.message} Automatic rollback failed: ${recovery.message}. Recovery state retained; run release-rollback.`); }
      throw new Error(`${error.message} Previous release restored; candidate retained.`);
    }
    if (state.superseded) {
      state.phase = 'retiring';
      this.write(state);
      return this.finishReplacement(state);
    }
    delete state.phase;
    this.write(state);
    return state;
  }

  finishReplacement(state) {
    this.ready(state.current);
    this.assertImage(state.previous);
    this.remove(state.superseded);
    delete state.superseded;
    delete state.phase;
    this.write(state);
    return state;
  }

  rollback() {
    const state = this.read();
    if (!state.previous) throw new Error('No previous release retained.');
    const canRestoreSuperseded = state.superseded && this.image(state.superseded.image, true);
    const target = canRestoreSuperseded ? state.superseded : state.previous;
    state.phase = 'rolling-back';
    this.write(state);
    this.start(target);
    this.run(['image', 'tag', target.image, `${repository}:experimental`]);
    if (canRestoreSuperseded) {
      state.candidate = state.current;
    } else state.previous = state.current;
    delete state.superseded;
    state.current = target;
    state.confirmed = false;
    delete state.phase;
    this.write(state);
    return state;
  }

  remove(entry) {
    const image = this.image(entry.image, true);
    if (!image) return;
    if (image.Id !== entry.id) throw new Error(`Immutable tag changed: ${entry.image}.`);
    if (this.inUse(entry.id)) throw new Error(`Image ${entry.image} is still referenced by a container. It was not removed.`);
    this.run(['image', 'rm', entry.image]);
  }

  inUse(id) {
    // Ancestor filters also match different image configurations sharing filesystem layers.
    const containers = this.run(['ps', '-aq']).split(/\s+/).filter(Boolean);
    if (!containers.length) return false;
    return this.run(['inspect', '--format', '{{.Image}}', ...containers]).split(/\s+/).includes(id);
  }

  confirm() {
    const state = this.read();
    if (state.phase) throw new Error('Recover the interrupted deployment first.');
    this.ready(state.current);
    if (state.previous) { this.remove(state.previous); state.previous = null; }
    state.confirmed = true;
    this.write(state);
    return state;
  }

  discard() {
    const state = this.read();
    if (!state.candidate) throw new Error('No candidate to discard.');
    this.remove(state.candidate);
    delete state.candidate;
    this.write(state);
    return state;
  }

  cleanImages() {
    const state = this.read();
    if (state.phase) throw new Error('Recover the interrupted deployment first.');
    for (const entry of [state.current, state.previous, state.candidate].filter(Boolean)) this.assertImage(entry);
    this.ready(state.current);
    // Existing fixture recipes use this alias. It must not retain a third runtime.
    this.run(['image', 'tag', state.current.image, `${repository}:experimental`]);
    const keep = new Set([state.current.image, state.previous?.image, state.candidate?.image, `${repository}:experimental`, 'yoink/rust-backend-fixture:experimental']);
    const lines = this.run(['image', 'ls', '--no-trunc', '--format', '{{json .}}']).split('\n').filter(Boolean).map(JSON.parse);
    const protectedIds = new Set(lines.filter(e => keep.has(`${e.Repository}:${e.Tag}`)).map(e => e.ID));
    for (const entry of lines) {
      const ref = `${entry.Repository}:${entry.Tag}`;
      const historical = [repository, 'yoink/rust-backend-fixture', 'yoink/rust-functional-runtime', 'yoink/rust-functional-fixture'].includes(entry.Repository)
        || (entry.Repository === 'yoink/rust-operations' && entry.Tag.startsWith('before-'));
      if (!historical || keep.has(ref) || entry.Tag === '<none>') continue;
      if (!protectedIds.has(entry.ID) && this.inUse(entry.ID)) {
        throw new Error(`Historical image ${ref} is still referenced by a container. Stop its owner before cleanup.`);
      }
      this.run(['image', 'rm', ref]);
      console.log(`Removed obsolete alias: ${ref}`);
    }
    return state;
  }

  cleanLegacyCache() {
    const state = this.read();
    const output = this.run(['buildx', 'du', '--builder', 'default', '--format=json']);
    const records = output.split('\n').filter(Boolean).map(JSON.parse);
    const ids = legacyCacheOrder(records);
    for (const id of ids) {
      this.run(['buildx', 'prune', '--builder', 'default', '--all', '--force', '--filter', `id=${id}`]);
    }
    console.log(`Removed ${ids.length} legacy Yoink cache references; unrelated cache was not selected.`);
    return state;
  }
}

async function main() {
  const root = fileURLToPath(new URL('../../', import.meta.url));
  const manager = new Releases(root);
  const [action, ...args] = process.argv.slice(2);
  const limits = { init: 3, status: 0, version: 1, build: 0, deploy: 0, rollback: 0, confirm: 0, discard: 0, trim: 0, cleanImages: 0, cleanLegacyCache: 0, tool: 1 };
  if (!Object.hasOwn(limits, action) || args.length > limits[action] || (action === 'init' && !args.length)) throw new Error('Usage: release.mjs init IMAGE [PREVIOUS VERSION] | status | version major|minor|patch | build | deploy | rollback | confirm | discard | trim | cleanImages | cleanLegacyCache | tool NAME');
  if (['build', 'tool'].includes(action)) await checkSpace(manager.backend);
  manager.locked(() => {
    if (action === 'version') {
      const file = path.join(manager.backend, 'VERSION');
      const next = bump(readFileSync(file, 'utf8').trim(), args[0]);
      writeFileSync(file, `${next}\n`); console.log(next); return;
    }
    if (action === 'trim') { manager.ensureBuilder(); manager.finishBuilder(); return; }
    if (action === 'tool') { manager.tool(args[0]); return; }
    const state = action === 'init' ? manager.initialize(...args) : action === 'status' ? manager.read() : manager[action]();
    console.log(JSON.stringify({ current: state.current.image, confirmed: state.confirmed, previous: state.previous?.image ?? null, candidate: state.candidate?.image ?? null, superseded: state.superseded?.image ?? null, phase: state.phase ?? null }, null, 2));
  });
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  handleInterrupts();
  main().catch(error => { console.error(error.message); process.exitCode = 1; });
}
