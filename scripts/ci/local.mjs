import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createReadStream, existsSync, lstatSync, mkdirSync, mkdtempSync, rmSync, statfsSync, writeFileSync, copyFileSync, chmodSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const HELPER_FILES = ['scripts/ci/Dockerfile', 'scripts/ci/controller.sh', 'scripts/ci/local.mjs', 'scripts/ci/local.test.mjs'];
export const BUILDKIT = 'moby/buildkit:v0.33.1@sha256:cec9f139f45e93c5c69c60f8b07cfad9f43f4ef6b6a6cd917527fea5ff2e3dea';
const HOST_SOCKET = 'unix:///var/run/docker.sock';
const SAFE_PATH = '/usr/local/bin:/usr/bin:/bin:/usr/local/sbin:/usr/sbin:/sbin';
const LOG_LIMIT = 512 * 1024;

export function isolatedEnv(home) {
  return { PATH: SAFE_PATH, HOME: home, LANG: 'C.UTF-8', GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null', GIT_TERMINAL_PROMPT: '0', DOCKER_CONFIG: path.join(home, 'docker') };
}

export function localJob(args) {
  assert(args.length <= 1 && ['verify', 'browser'].includes(args[0] ?? 'verify'), 'Usage: just ci-local [verify|browser]');
  return args[0] ?? 'verify';
}

export function requireStorageSpace(directory, inspect = statfsSync) {
  const fs = inspect(directory, { bigint: true });
  assert(fs.bavail * fs.bsize >= 40n * 1024n ** 3n, `Local CI needs at least 40 GiB free on the filesystem containing ${directory}.`);
}

export async function imageWasAbsent(docker, reference) {
  try {
    await docker(['image', 'inspect', '--format', '{{.Id}}', reference]);
    return false;
  } catch (error) {
    if (/No such image:/i.test(error.stderr ?? '')) return true;
    throw error;
  }
}

export function resilientOutput(stream) {
  // File logging and cleanup must survive an output consumer closing the pipe.
  let closed = false;
  stream.on('error', () => { closed = true; });
  return chunk => {
    if (!closed && !stream.destroyed) stream.write(chunk);
  };
}

function git(cwd, env, args) {
  const result = spawnSync('git', ['-c', 'core.hooksPath=/dev/null', '-c', 'core.fsmonitor=false', '-c', 'init.templateDir=', ...args], {
    cwd, env, encoding: 'utf8', timeout: 120000, maxBuffer: 32 * 1024 ** 2,
  });
  assert(!result.error && result.status === 0, `git ${args[0]} failed: ${result.error?.message ?? result.stderr}`);
  return result.stdout.trimEnd();
}

export function humanIdentity(source) {
  // Read only the configured commit identity. No user config enters the clone.
  const env = { PATH: SAFE_PATH, HOME: os.userInfo().homedir };
  const values = ['user.name', 'user.email'].map(key => {
    const result = spawnSync('git', ['config', '--get', key], { cwd: source, env, encoding: 'utf8', timeout: 10000 });
    assert(result.status === 0 && result.stdout.trim(), `Configure ${key} before local CI.`);
    return result.stdout.trim();
  });
  return { name: values[0], email: values[1] };
}

export function safeSourcePath(relative) {
  assert(relative && !path.isAbsolute(relative) && !relative.split('/').some(p => ['', '.', '..', '.git'].includes(p)), `Unsafe source path: ${relative}`);
  for (const part of relative.split('/')) {
    assert(!((part === '.env' || part.startsWith('.env.')) && part !== '.env.example') &&
      !['.actrc', '.secrets', '.vars', '.input', '.docker', '.npmrc', '.netrc', '.gitconfig', '.git-credentials'].includes(part),
    `Refusing tracked runtime configuration: ${relative}`);
  }
  return relative;
}

function sourceInfo(source, relative) {
  let current = source;
  for (const part of relative.split('/')) {
    current = path.join(current, part);
    assert(existsSync(current), `Missing tracked file or submodule: ${current}`);
    assert(!lstatSync(current).isSymbolicLink(), `Source symlinks require review before local CI: ${current}`);
  }
  return lstatSync(current);
}

function entries(source, env) {
  return git(source, env, ['ls-files', '--stage', '-z']).split('\0').filter(Boolean).map(line => {
    const match = /^(\d+) ([a-f0-9]+) (\d)\t([\s\S]+)$/.exec(line);
    assert(match && match[3] === '0', 'Resolve index conflicts before local CI.');
    return { mode: match[1], sha: match[2], file: safeSourcePath(match[4]) };
  });
}

function validateRepository(source, env, submodule = false, expected = '') {
  assert(git(source, env, ['rev-parse', '--is-shallow-repository']) === 'false', `Full history required: ${source}`);
  if (submodule) {
    assert(git(source, env, ['rev-parse', 'HEAD']) === expected, `Submodule commit differs from index: ${source}`);
    assert(git(source, env, ['status', '--porcelain', '--untracked-files=all', '--ignore-submodules=none']) === '', `Dirty submodule: ${source}`);
  }
  const files = entries(source, env);
  for (const entry of files) {
    const absolute = path.join(source, entry.file);
    const info = sourceInfo(source, entry.file);
    if (entry.mode === '160000') {
      assert(existsSync(path.join(absolute, '.git')), `Uninitialized submodule: ${absolute}`);
      validateRepository(absolute, env, true, entry.sha);
    } else {
      assert(info.isFile() && ['100644', '100755'].includes(entry.mode), `Unsupported tracked file: ${absolute}`);
    }
  }
  return files;
}

function mirrorRepository(source, destination, env) {
  mkdirSync(destination, { recursive: true });
  git(source, env, ['clone', '--quiet', '--mirror', '--no-local', source, path.join(destination, '.git')]);
  git(destination, env, ['config', 'core.bare', 'false']);
  git(destination, env, ['config', '--unset', 'remote.origin.mirror']);
  git(destination, env, ['config', 'remote.origin.fetch', '+refs/heads/*:refs/remotes/origin/*']);
  const origin = destination.endsWith('/source') ? 'https://github.com/AdamsGH/yoink-core.git' :
    `https://github.com/AdamsGH/${path.basename(destination)}.git`;
  git(destination, env, ['remote', 'set-url', 'origin', origin]);
  git(destination, env, ['reset', '--mixed', '--quiet', 'HEAD']);
  for (const { file, mode } of entries(source, env)) {
    const target = path.join(destination, file);
    if (mode === '160000') mirrorRepository(path.join(source, file), target, env);
    else {
      mkdirSync(path.dirname(target), { recursive: true });
      copyFileSync(path.join(source, file), target);
      chmodSync(target, lstatSync(path.join(source, file)).mode & 0o100 ? 0o755 : 0o644);
    }
  }
}

export function createSnapshot(source, destination, env, identity, helpers = HELPER_FILES) {
  validateRepository(source, env);
  for (const relative of helpers) {
    safeSourcePath(relative);
    assert(sourceInfo(source, relative).isFile(), `Missing helper: ${relative}`);
  }
  mirrorRepository(source, destination, env);
  for (const relative of helpers) {
    const target = path.join(destination, relative);
    mkdirSync(path.dirname(target), { recursive: true });
    copyFileSync(path.join(source, relative), target);
  }
  git(destination, env, ['config', 'user.name', identity.name]);
  git(destination, env, ['config', 'user.email', identity.email]);
  git(destination, env, ['add', '--all']);
  git(destination, env, ['-c', 'commit.gpgSign=false', 'commit', '--quiet', '--allow-empty', '-m', 'Snapshot local CI source']);
  git(destination, env, ['config', '--unset', 'user.name']);
  git(destination, env, ['config', '--unset', 'user.email']);
  assert(git(destination, env, ['status', '--porcelain']) === '', 'Snapshot is not clean.');
  return git(destination, env, ['rev-parse', 'HEAD']);
}

export function controllerArgs(state, source, job = 'verify') {
  return ['create', '--name', state.controller, '--label', `io.yoink.ci-local=${state.id}`, '--interactive',
    '--privileged', '--cgroupns=private', '--network', state.network, '--cpus=4', '--memory=8g', '--memory-swap=8g', '--pids-limit=8192', '--shm-size=1g',
    '--mount', `type=volume,src=${state.volume},dst=/var/lib/docker`,
    '--mount', `type=bind,src=${source},dst=${source}`, '--user=0:0', '--entrypoint', 'bash', state.image,
    `${source}/scripts/ci/controller.sh`, source, state.image, localJob([job])];
}

export async function cleanupOwned(state, docker, removeDirectory) {
  const failures = [];
  const attempt = async (key, args) => {
    if (!state[key]) return;
    try { await docker(args); state[key] = false; } catch (error) {
      if (/no such (container|image|volume|network)\b|no builder .* found|volume .* not found|network .* not found/i.test(error.stderr ?? '')) state[key] = false;
      else failures.push(`${key}: ${error.message}`);
    }
  };
  await attempt('hasController', ['rm', '--force', '--volumes', state.controller]);
  await attempt('hasBuilder', ['buildx', 'rm', '--force', state.builder]);
  await attempt('hasVolume', ['volume', 'rm', state.volume]);
  await attempt('hasNetwork', ['network', 'rm', state.network]);
  if (state.needsOwnership && !state.hasController) {
    try {
      state.hasOwnerHelper = true;
      await docker(['run', '--rm', '--name', `${state.id}-owner`, '--network=none', '--read-only', '--user=0:0',
        '--mount', `type=bind,src=${state.checkout},dst=${state.checkout}`, '--entrypoint', 'chown', state.image,
        '-R', state.owner, state.checkout]);
      state.needsOwnership = false;
      state.hasOwnerHelper = false;
    } catch (error) { failures.push(`snapshot ownership: ${error.message}`); }
    await attempt('hasOwnerHelper', ['rm', '--force', `${state.id}-owner`]);
  }
  if (!state.needsOwnership && !state.hasOwnerHelper) await attempt('hasImage', ['image', 'rm', state.image]);
  else failures.push(`Preserved recovery image ${state.image}`);
  await attempt('ownsBuildkitImage', ['image', 'rm', BUILDKIT]);
  // Never unlink a checkout or context while a container might still use it.
  if (!state.hasController && !state.hasBuilder && !state.hasOwnerHelper && !state.needsOwnership) {
    try { removeDirectory(state.directory); } catch (error) { failures.push(`temporary files: ${error.message}`); }
  } else failures.push(`Preserved temporary files at ${state.directory}`);
  if (failures.length) throw new Error(`Local CI cleanup incomplete:\n${failures.join('\n')}`);
}

export async function main() {
  const job = localJob(process.argv.slice(2));
  assert(process.platform === 'linux' && process.arch === 'x64', 'Local CI requires Linux amd64.');
  const source = fileURLToPath(new URL('../../', import.meta.url));
  const identity = humanIdentity(source);
  const workRoot = path.join(source, 'backend/target');
  mkdirSync(workRoot, { recursive: true });
  assert(!lstatSync(workRoot).isSymbolicLink(), 'Refusing symlink build directory.');
  const directory = mkdtempSync(path.join(workRoot, 'yoink-ci-local-'));
  const id = path.basename(directory).toLowerCase();
  const state = { id, directory, owner: `${os.userInfo().uid}:${os.userInfo().gid}`, image: `${id}:runner`, controller: `${id}-controller`, builder: `${id}-builder`, network: `${id}-network`, volume: `${id}-docker` };
  const home = path.join(directory, 'home');
  const env = isolatedEnv(home);
  const logDirectory = path.join(source, 'backend/target/ci-local-logs');
  const logFile = path.join(logDirectory, `${id}.log`);
  let tail = Buffer.alloc(0);
  let active;
  let interrupted = 0;
  let cleaning = false;
  let logError;
  let logReady = false;
  const writeOutput = resilientOutput(process.stdout);
  const log = chunk => {
    writeOutput(chunk);
    tail = Buffer.concat([tail, Buffer.from(chunk)]).subarray(-LOG_LIMIT);
    if (!logReady) return;
    try { writeFileSync(logFile, tail, { mode: 0o600 }); } catch (error) {
      logError ??= error;
      if (!cleaning) active?.kill('SIGTERM');
    }
  };
  const run = (command, args, input) => new Promise((resolve, reject) => {
    if (!cleaning && (interrupted || logError)) return reject(logError ?? new Error('Local CI interrupted.'));
    const child = spawn(command, args, { cwd: directory, env, stdio: [input ? 'pipe' : 'ignore', 'pipe', 'pipe'] });
    active = child;
    let output = '';
    let stderr = '';
    let inputError;
    const timer = setTimeout(() => child.kill('SIGKILL'), cleaning ? 120000 : 90 * 60 * 1000);
    child.stdout.on('data', chunk => { output = (output + chunk).slice(-LOG_LIMIT); log(chunk); });
    child.stderr.on('data', chunk => { stderr = (stderr + chunk).slice(-LOG_LIMIT); log(chunk); });
    child.once('error', reject);
    if (input) {
      input.on('error', error => { inputError = error; child.kill('SIGTERM'); });
      child.stdin.on('error', error => { inputError = error; });
      input.pipe(child.stdin);
    }
    child.once('close', (code, signal) => {
      clearTimeout(timer);
      if (active === child) active = undefined;
      input?.destroy();
      if (logError && !cleaning) reject(logError);
      else if (code === 0 && !inputError) resolve(output.trim());
      else reject(Object.assign(new Error(`${command} failed (${signal ?? code}): ${inputError?.message ?? stderr.slice(-2000)}`), { stderr }));
    });
  });
  const docker = (args, input) => run('docker', ['--host', HOST_SOCKET, '--config', env.DOCKER_CONFIG, ...args], input);
  const onInt = () => { interrupted ||= 130; if (!cleaning) active?.kill('SIGTERM'); };
  const onTerm = () => { interrupted ||= 143; if (!cleaning) active?.kill('SIGTERM'); };
  process.on('SIGINT', onInt);
  process.on('SIGTERM', onTerm);
  try {
    mkdirSync(home);
    mkdirSync(logDirectory, { recursive: true });
    assert(!lstatSync(logDirectory).isSymbolicLink(), 'Refusing symlink log directory.');
    writeFileSync(logFile, '', { flag: 'wx', mode: 0o600 });
    logReady = true;
    requireStorageSpace(directory);
    const checkout = path.join(directory, 'source');
    state.checkout = checkout;
    const sha = createSnapshot(source, checkout, env, identity);
    log(`Local CI snapshot: ${sha}\nLog tail: ${logFile}\n`);
    const context = path.join(directory, 'context');
    mkdirSync(context);
    copyFileSync(path.join(checkout, 'scripts/ci/Dockerfile'), path.join(context, 'Dockerfile'));
    const daemon = JSON.parse(await docker(['info', '--format', '{"OSType":{{json .OSType}},"Architecture":{{json .Architecture}},"DockerRootDir":{{json .DockerRootDir}},"DriverStatus":{{json .DriverStatus}}}']));
    assert(daemon.OSType === 'linux' && daemon.Architecture === 'x86_64', 'Local Linux amd64 Docker daemon required.');
    assert(path.isAbsolute(daemon.DockerRootDir), 'Docker storage must be on the local host.');
    requireStorageSpace(daemon.DockerRootDir);
    if (daemon.DriverStatus?.some(([key, value]) => key === 'driver-type' && value === 'io.containerd.snapshotter.v1')) {
      requireStorageSpace('/var/lib/containerd');
    }
    state.ownsBuildkitImage = await imageWasAbsent(docker, BUILDKIT);
    state.hasBuilder = true;
    await docker(['buildx', 'create', '--name', state.builder, '--driver', 'docker-container', '--driver-opt',
      `image=${BUILDKIT},network=bridge,memory=2g,memory-swap=2g,cpu-period=100000,cpu-quota=200000`]);
    state.hasImage = true;
    await docker(['buildx', 'build', '--builder', state.builder, '--platform', 'linux/amd64', '--load', '--tag', state.image, context]);
    const archive = path.join(directory, 'runner.tar');
    await docker(['image', 'save', '--output', archive, state.image]);
    state.hasNetwork = true;
    await docker(['network', 'create', '--driver', 'bridge', '--label', `io.yoink.ci-local=${id}`, state.network]);
    state.hasVolume = true;
    await docker(['volume', 'create', '--label', `io.yoink.ci-local=${id}`, state.volume]);
    state.hasController = true;
    state.needsOwnership = true;
    await docker(controllerArgs(state, checkout, job));
    await docker(['start', '--attach', '--interactive', state.controller], createReadStream(archive));
    const code = await docker(['inspect', '--format', '{{.State.ExitCode}}', state.controller]);
    assert(code === '0', `Local workflow failed with controller exit ${code}.`);
    log(`Local ci.yml ${job} job passed. No publishing or deployment job selected.\n`);
  } catch (error) {
    log(`Local CI failed: ${error.message}\n`);
    throw error;
  } finally {
    cleaning = true;
    try {
      await cleanupOwned(state, docker, target => rmSync(target, { recursive: true, force: true }));
    } finally {
      process.off('SIGINT', onInt);
      process.off('SIGTERM', onTerm);
      writeOutput(`${logReady ? `Local CI log tail: ${logFile}` : 'Local CI log could not be created.'}\n`);
      if (interrupted) process.exitCode = interrupted;
    }
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch(error => { console.error(error.message); process.exitCode ||= 1; });
}
