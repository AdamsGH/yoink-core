import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, copyFileSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { Writable } from 'node:stream';
import { BUILDKIT, cleanupOwned, controllerArgs, createSnapshot, HELPER_FILES, humanIdentity, imageWasAbsent, isolatedEnv, localJob, requireStorageSpace, resilientOutput, safeSourcePath } from './local.mjs';

const project = fileURLToPath(new URL('../../', import.meta.url));
const identity = { name: 'CI Fixture', email: 'fixture@example.invalid' };

function fixture(t) {
  const directory = mkdtempSync(path.join(os.tmpdir(), 'yoink-ci-test-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const source = path.join(directory, 'original');
  const home = path.join(directory, 'home');
  mkdirSync(source);
  mkdirSync(home);
  const env = isolatedEnv(home);
  const git = (cwd, ...args) => {
    const result = spawnSync('git', ['-c', 'init.templateDir=', '-c', 'core.hooksPath=/dev/null', ...args], { cwd, env, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    return result.stdout.trim();
  };
  const init = dir => {
    git(dir, 'init', '--quiet', '--initial-branch=master');
    git(dir, 'config', 'user.name', identity.name);
    git(dir, 'config', 'user.email', identity.email);
  };
  const commit = (dir, message) => {
    git(dir, 'add', '--all');
    git(dir, '-c', 'commit.gpgSign=false', 'commit', '--quiet', '-m', message);
    return git(dir, 'rev-parse', 'HEAD');
  };
  init(source);
  writeFileSync(path.join(source, '.gitignore'), '.env\n.secrets\nignored/\n');
  writeFileSync(path.join(source, 'tracked.txt'), 'committed\n');
  writeFileSync(path.join(source, '.env.example'), 'EXAMPLE=not-a-secret\n');
  const original = commit(source, 'Test local CI snapshot');
  const destination = path.join(directory, 'source');
  return { directory, source, destination, env, git, init, commit, original };
}

test('snapshot includes tracked edits and named helpers, never ignored or arbitrary untracked files', t => {
  const f = fixture(t);
  f.git(f.source, 'branch', 'history-only');
  writeFileSync(path.join(f.source, 'tracked.txt'), 'local edit\n');
  chmodSync(path.join(f.source, 'tracked.txt'), 0o755);
  for (const name of ['.env', '.secrets', '.actrc', 'untracked.txt']) writeFileSync(path.join(f.source, name), 'DO_NOT_COPY=private\n');
  mkdirSync(path.join(f.source, 'scripts'));
  writeFileSync(path.join(f.source, 'scripts/helper.mjs'), '// explicit new helper\n');
  createSnapshot(f.source, f.destination, f.env, identity, ['scripts/helper.mjs']);
  assert.equal(readFileSync(path.join(f.destination, 'tracked.txt'), 'utf8'), 'local edit\n');
  assert.equal(lstatSync(path.join(f.destination, 'tracked.txt')).mode & 0o111, 0o111);
  assert.equal(readFileSync(path.join(f.destination, 'scripts/helper.mjs'), 'utf8'), '// explicit new helper\n');
  assert(existsSync(path.join(f.destination, '.env.example')));
  for (const name of ['.env', '.secrets', '.actrc', 'untracked.txt']) assert(!existsSync(path.join(f.destination, name)));
  assert.equal(f.git(f.destination, 'status', '--porcelain'), '');
  assert.equal(f.git(f.source, 'rev-parse', 'HEAD'), f.original);
  assert.equal(f.git(f.destination, 'rev-parse', 'history-only'), f.original);
  assert.equal(f.git(f.destination, 'show', `${f.original}:tracked.txt`), 'committed');
  assert.equal(f.git(f.destination, 'config', '--get', 'remote.origin.url'), 'https://github.com/AdamsGH/yoink-core.git');
  assert(!existsSync(path.join(f.destination, '.git/objects/info/alternates')));
  const configured = spawnSync('git', ['config', '--get', 'user.name'], { cwd: f.destination, env: f.env });
  assert.equal(configured.status, 1);
});

test('snapshot commit identity comes from the configured source repository', t => {
  const f = fixture(t);
  assert.deepEqual(humanIdentity(f.source), identity);
});

test('snapshot ignores non-owner executable permission bits like Git', t => {
  const f = fixture(t);
  chmodSync(path.join(f.source, 'tracked.txt'), 0o654);
  createSnapshot(f.source, f.destination, f.env, identity, []);
  assert.equal(lstatSync(path.join(f.destination, 'tracked.txt')).mode & 0o777, 0o644);
});

test('snapshot preserves staged deletion without changing the original index', t => {
  const f = fixture(t);
  f.git(f.source, 'rm', '--quiet', 'tracked.txt');
  const before = f.git(f.source, 'status', '--porcelain');
  createSnapshot(f.source, f.destination, f.env, identity, []);
  assert(!existsSync(path.join(f.destination, 'tracked.txt')));
  assert.equal(f.git(f.source, 'status', '--porcelain'), before);
});

test('missing tracked files and source symlinks fail closed', t => {
  const f = fixture(t);
  rmSync(path.join(f.source, 'tracked.txt'));
  assert.throws(() => createSnapshot(f.source, f.destination, f.env, identity, []), /Missing tracked file/);
  symlinkSync('/etc/passwd', path.join(f.source, 'tracked.txt'));
  assert.throws(() => createSnapshot(f.source, f.destination, f.env, identity, []), /symlinks require review/);
  assert(!existsSync(f.destination));
});

test('full submodule histories are independent and dirty submodules fail closed', t => {
  const f = fixture(t);
  const plugin = path.join(f.directory, 'plugin');
  mkdirSync(plugin);
  f.init(plugin);
  writeFileSync(path.join(plugin, '.gitignore'), '.env\n');
  writeFileSync(path.join(plugin, 'plugin.txt'), 'old\n');
  const old = f.commit(plugin, 'Test plugin history');
  writeFileSync(path.join(plugin, 'plugin.txt'), 'new\n');
  f.commit(plugin, 'Test plugin update');
  f.git(f.source, '-c', 'protocol.file.allow=always', 'submodule', 'add', '--quiet', plugin, 'plugins/test-plugin');
  f.commit(f.source, 'Test submodule history');
  const localPlugin = path.join(f.source, 'plugins/test-plugin');
  writeFileSync(path.join(localPlugin, '.env'), 'PRIVATE=do-not-copy\n');
  createSnapshot(f.source, f.destination, f.env, identity, []);
  const copy = path.join(f.destination, 'plugins/test-plugin');
  assert.equal(f.git(copy, 'show', `${old}:plugin.txt`), 'old');
  assert(lstatSync(path.join(copy, '.git')).isDirectory());
  assert(!existsSync(path.join(copy, '.git/objects/info/alternates')));
  assert(!existsSync(path.join(copy, '.env')));
  writeFileSync(path.join(localPlugin, 'plugin.txt'), 'dirty\n');
  assert.throws(() => createSnapshot(f.source, path.join(f.directory, 'dirty'), f.env, identity, []), /Dirty submodule/);
  assert(!existsSync(path.join(f.directory, 'dirty')));
});

test('environment and tracked runtime configuration cannot inject credentials or act options', () => {
  const env = isolatedEnv('/disposable/home');
  for (const key of ['GITHUB_TOKEN', 'GH_TOKEN', 'DOCKER_HOST', 'DOCKER_CONTEXT', 'HTTP_PROXY', 'NODE_OPTIONS', 'BASH_ENV', 'SSH_AUTH_SOCK']) assert.equal(env[key], undefined);
  assert.equal(env.GIT_CONFIG_GLOBAL, '/dev/null');
  assert.equal(env.DOCKER_CONFIG, '/disposable/home/docker');
  for (const file of ['.env', '.env.local', '.actrc', '.secrets', 'frontend/.npmrc', '../production', '.git/config']) assert.throws(() => safeSourcePath(file));
  assert.equal(safeSourcePath('.env.example'), '.env.example');
});

test('controller exposes only a disposable checkout and private Docker data volume', () => {
  const state = { id: 'owned', controller: 'owned-controller', image: 'owned:runner', network: 'owned-bridge', volume: 'owned-docker' };
  const args = controllerArgs(state, '/tmp/owned/source');
  assert.equal(args.filter(a => a.startsWith('type=bind,')).length, 1);
  assert(args.includes('type=bind,src=/tmp/owned/source,dst=/tmp/owned/source'));
  assert(args.includes('type=volume,src=owned-docker,dst=/var/lib/docker'));
  assert(!args.some(a => /docker\.sock|network[= ]host/.test(a)));
  assert.equal(args[args.indexOf('--network') + 1], 'owned-bridge');
  assert(args.includes('--privileged'));
  assert(args.includes('--cgroupns=private'));
  assert(args.includes('--memory=8g'));
});

test('real workflow selection, empty secret files, sandbox and inner socket remain mandatory', () => {
  const controller = readFileSync(new URL('./controller.sh', import.meta.url), 'utf8');
  assert.match(controller, /act pull_request --workflows \.github\/workflows\/ci\.yml --job "\$job"/);
  for (const option of ['env', 'secret', 'var', 'input']) assert(controller.includes(`--${option}-file /ci/empty`));
  assert.match(controller, /--container-daemon-socket \/var\/run\/docker\.sock/);
  assert.match(controller, /test ! -e \/var\/run\/docker\.sock/);
  assert.match(controller, /--no-cache-server/);
  assert.match(controller, /--user=10001:10001/);
  assert.match(controller, /Seccomp-BPF sandbox/);
  assert(!controller.includes('--no-sandbox'));
  assert(!controller.includes('--network host'));
  assert(!controller.includes('--no-skip-checkout'));
  assert(!controller.includes('--replace'));
  for (const file of HELPER_FILES) assert(existsSync(path.join(project, file)), file);
});

test('local selection permits the complete source gate or browser preflight, never publication', () => {
  assert.equal(localJob([]), 'verify');
  assert.equal(localJob(['verify']), 'verify');
  assert.equal(localJob(['browser']), 'browser');
  for (const args of [['containers'], ['required'], ['push'], ['browser', '--secret=TOKEN']]) assert.throws(() => localJob(args), /Usage/);
});

test('storage checks do not confuse a roomy checkout with a full Docker filesystem', () => {
  const stats = directory => ({ bavail: directory === '/checkout' ? 80n : 5n, bsize: 1024n ** 3n });
  assert.doesNotThrow(() => requireStorageSpace('/checkout', stats));
  assert.throws(() => requireStorageSpace('/var/lib/docker', stats), /40 GiB.*\/var\/lib\/docker/);
  assert.throws(() => requireStorageSpace('/var/lib/containerd', stats), /40 GiB.*\/var\/lib\/containerd/);
});

test('a rejected log directory cannot create files through its symlink in the error handler', t => {
  const f = fixture(t);
  const script = path.join(f.source, 'scripts/ci/local.mjs');
  mkdirSync(path.dirname(script), { recursive: true });
  copyFileSync(new URL('./local.mjs', import.meta.url), script);
  const target = path.join(f.source, 'backend/target');
  const outside = path.join(f.directory, 'outside');
  mkdirSync(target, { recursive: true });
  mkdirSync(outside);
  symlinkSync(outside, path.join(target, 'ci-local-logs'));
  const result = spawnSync(process.execPath, [script, 'browser'], { env: f.env, encoding: 'utf8', timeout: 10000 });
  assert.ifError(result.error);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Refusing symlink log directory/);
  assert.deepEqual(readdirSync(outside), []);
  assert.deepEqual(readdirSync(target), ['ci-local-logs']);
});

test('an ambiguous Docker inspection failure never authorizes deletion of a shared image', async () => {
  assert.equal(await imageWasAbsent(async () => 'existing-image-id', BUILDKIT), false);
  assert.equal(await imageWasAbsent(async () => { throw Object.assign(new Error('missing'), { stderr: 'Error response from daemon: No such image: buildkit' }); }, BUILDKIT), true);
  const state = { directory: '/owned' };
  await assert.rejects(async () => {
    state.ownsBuildkitImage = await imageWasAbsent(async () => { throw Object.assign(new Error('unavailable'), { stderr: 'Cannot connect to the Docker daemon' }); }, BUILDKIT);
  }, /unavailable/);
  await cleanupOwned(state, () => assert.fail('no image is owned'), () => {});
});

test('a closed output pipe cannot terminate cleanup or trigger more writes', async () => {
  let writes = 0;
  const stream = new Writable({ write(_chunk, _encoding, done) {
    writes++;
    done(Object.assign(new Error('consumer closed'), { code: 'EPIPE' }));
  } });
  const output = resilientOutput(stream);
  const closed = new Promise(resolve => stream.once('close', resolve));
  output('first');
  await closed;
  output('second');
  let cleaned = false;
  await cleanupOwned({ directory: '/owned' }, () => assert.fail('no Docker resources are owned'), () => { cleaned = true; });
  assert.equal(cleaned, true);
  assert.equal(writes, 1);
});

function ownedState() {
  return { id: 'owned', directory: '/tmp/owned', checkout: '/tmp/owned/source', owner: '1000:1000', controller: 'owned-controller', builder: 'owned-builder', image: 'owned:runner', volume: 'owned-volume', network: 'owned-network',
    hasController: true, hasBuilder: true, hasVolume: true, hasNetwork: true, hasImage: true, ownsBuildkitImage: true, needsOwnership: true };
}

test('cleanup removes only owned resources and restores snapshot ownership before unlinking', async () => {
  const state = ownedState();
  const calls = [];
  await cleanupOwned(state, async args => { calls.push(args); }, dir => calls.push(['unlink', dir]));
  assert.deepEqual(calls[0], ['rm', '--force', '--volumes', 'owned-controller']);
  const owner = calls.findIndex(args => args[0] === 'run');
  assert(owner > 0 && calls[owner].includes('1000:1000'));
  assert(owner < calls.findIndex(args => args[0] === 'image'));
  assert.deepEqual(calls.at(-1), ['unlink', '/tmp/owned']);
  assert(!calls.some(args => args.includes('prune')));
  assert.equal(state.needsOwnership, false);
});

test('cleanup continues after failure but never deletes a possibly mounted checkout', async () => {
  const state = ownedState();
  const calls = [];
  await assert.rejects(cleanupOwned(state, async args => {
    calls.push(args);
    if (args.includes('owned-controller')) throw new Error('daemon unavailable');
  }, () => assert.fail('must preserve mounted checkout')), /cleanup incomplete/);
  assert(calls.some(args => args.includes('owned-builder')));
  assert(calls.some(args => args.includes('owned-volume')));
  assert(!calls.some(args => args[0] === 'run'));
});

test('cleanup handles partial startup and reports ownership recovery failures', async () => {
  const state = ownedState();
  await assert.rejects(cleanupOwned(state, async args => {
    if (args[0] === 'rm') throw Object.assign(new Error('already gone'), { stderr: 'Error response from daemon: No such container: owned-controller' });
    if (args[0] === 'run') throw new Error('cannot restore ownership');
  }, () => assert.fail('must preserve inaccessible files')), /snapshot ownership: cannot restore ownership/);
  assert.equal(state.hasController, false);
  assert.equal(state.needsOwnership, true);
});
