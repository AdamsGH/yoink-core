import { spawn } from 'node:child_process';
import { lstatSync, rmSync } from 'node:fs';
import { setTimeout as delay } from 'node:timers/promises';
import { pathToFileURL } from 'node:url';
import { dirname } from 'node:path';
import { healthy } from './health.mjs';

export function settings(env = process.env) {
  const integer = (name, fallback, min, max) => {
    const value = Number(env[name] ?? fallback);
    if (!Number.isSafeInteger(value) || value < min || value > max) throw new Error(`invalid ${name}`);
    return value;
  };
  return {
    startup: integer('YOINK_YOUTUBE_STARTUP_TIMEOUT_MS', 30000, 100, 300000),
    shutdown: integer('YOINK_YOUTUBE_SHUTDOWN_TIMEOUT_MS', 150000, 100, 150000),
  };
}

export function supervise(specs, { startup, shutdown }, readiness) {
  const children = new Map();
  let stopping = false;
  let timer;
  let result = 0;
  let finish;
  const done = new Promise(resolve => { finish = resolve; });
  const signal = (child, name) => {
    if (!child.pid) return;
    try { process.kill(-child.pid, name); } catch (error) { if (error.code !== 'ESRCH') throw error; }
  };
  const terminate = except => {
    for (const [name, child] of children) if (name !== except) signal(child, 'SIGTERM');
  };
  const stop = (code = 0, drain = true) => {
    if (stopping) return;
    stopping = true;
    result = code;
    timer = setTimeout(() => {
      for (const child of children.values()) signal(child, 'SIGKILL');
    }, shutdown);
    if (drain && children.has('gateway')) signal(children.get('gateway'), 'SIGTERM');
    else terminate();
    if (!children.size) { clearTimeout(timer); finish(result); }
  };
  for (const spec of specs) {
    // Upstream errors can contain OAuth credentials. Log only process state.
    const child = spawn(spec.command ?? process.execPath, spec.args, { cwd: spec.cwd, env: spec.env ?? process.env, detached: true, stdio: 'ignore' });
    children.set(spec.name, child);
    let exited = false;
    const exit = code => {
      if (exited) return;
      exited = true;
      signal(child, 'SIGKILL');
      children.delete(spec.name);
      if (!stopping) {
        console.error(`YouTube component ${spec.name} exited (${code ?? 'signal'}).`);
        stop(1, false);
      } else if (spec.name === 'gateway') terminate();
      if (!children.size) { clearTimeout(timer); finish(result); }
    };
    child.once('error', () => exit(1));
    child.once('exit', exit);
  }
  const ready = (async () => {
    const deadline = Date.now() + startup;
    while (!stopping) {
      try { await readiness(); return; } catch {}
      if (Date.now() >= deadline) { stop(1, false); throw new Error('YouTube startup timed out'); }
      await delay(100);
    }
    throw new Error('YouTube startup interrupted');
  })();
  return { ready, done, stop };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const socket = process.env.YOINK_YOUTUBE_AUTH_SOCKET;
  if (!socket?.startsWith('/')) throw new Error('invalid YouTube socket');
  const directory = lstatSync(dirname(socket));
  if (directory.isSymbolicLink() || directory.uid !== process.getuid() || (directory.mode & 0o077)) throw new Error('YouTube socket directory must be private and owned by the service user');
  rmSync(socket, { force: true });
  const service = supervise([
    { name: 'youtubei', cwd: '/opt/youtubei', args: ['index.js'] },
    { name: 'bgutil', cwd: '/app', args: ['build/main.js', '--host', '127.0.0.1'] },
    { name: 'gateway', args: ['--import', '/opt/limits.mjs', '/opt/adapter.mjs'] },
  ], settings(), healthy);
  process.once('SIGTERM', () => service.stop());
  process.once('SIGINT', () => service.stop());
  service.ready.catch(() => {});
  process.exitCode = await service.done;
}
