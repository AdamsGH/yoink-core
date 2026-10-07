import { createWriteStream, statSync } from 'node:fs';
import { Readable, Transform } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { spawn } from 'node:child_process';

export function config(env = process.env) {
  const integer = (key, fallback, min, max) => {
    const value = env[key] ?? String(fallback);
    if (!/^\d+$/.test(value) || !Number.isSafeInteger(Number(value)) || Number(value) < min || Number(value) > max) throw new Error(`invalid ${key}`);
    return Number(value);
  };
  return {
    evalTimeoutMs: integer('JS_EVAL_TIMEOUT_MS', 1000, 10, 5000),
    port: integer('PORT', 9173, 1, 65535),
    concurrency: integer('MAX_CONCURRENCY', 2, 1, 16),
    // Deliberately no in-memory wait queue: overload fails closed immediately.
    queue: integer('MAX_QUEUE', 0, 0, 0),
    deadlineMs: integer('JOB_DEADLINE_MS', 240000, 10, 270000),
    killGraceMs: integer('FFMPEG_KILL_GRACE_MS', 1000, 10, 5000),
    maxFileBytes: integer('MAX_FILE_BYTES', 67108864, 1, 1073741824),
    maxScratchBytes: integer('MAX_SCRATCH_BYTES', 201326592, 1, 3221225472),
  };
}

export function job(config) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), config.deadlineMs);
  let bytes = 0;
  return {
    signal: controller.signal,
    abort: () => controller.abort(),
    dispose: () => clearTimeout(timer),
    check() { controller.signal.throwIfAborted(); },
    account(n, fileBytes) {
      bytes += n;
      if (fileBytes > config.maxFileBytes || bytes > config.maxScratchBytes) throw new Error('byte limit');
    },
    fetch: (input, init = {}) => fetch(input, { ...init, signal: init.signal ? AbortSignal.any([init.signal, controller.signal]) : controller.signal }),
    async run(promise) {
      controller.signal.throwIfAborted();
      let listener;
      try {
        return await Promise.race([promise, new Promise((_, reject) => {
          listener = () => reject(new Error('job aborted'));
          controller.signal.addEventListener('abort', listener, { once: true });
        })]);
      } finally { controller.signal.removeEventListener('abort', listener); }
    },
    async streamToFile(stream, path) {
      let fileBytes = 0;
      const limiter = new Transform({ transform: (chunk, _, cb) => {
        try { fileBytes += chunk.length; this.account(chunk.length, fileBytes); cb(null, chunk); } catch (error) { cb(error); }
      }});
      const source = stream instanceof Readable ? stream : Readable.fromWeb(stream);
      await pipeline(source, limiter, createWriteStream(path, { flags: 'wx', mode: 0o600 }), { signal: controller.signal });
    },
    async ffmpeg(args, output) {
      this.check();
      if (bytes >= config.maxScratchBytes) throw new Error('byte limit');
      // ffmpeg's own output ceiling prevents an unbounded output between polls.
      const proc = spawn('ffmpeg', ['-nostdin', '-filter_threads', '1', ...args.slice(0, -1), '-threads', '1', '-fs', String(Math.min(config.maxFileBytes, config.maxScratchBytes - bytes)), args.at(-1)], { stdio: 'ignore' });
      let killTimer;
      const abort = () => {
        proc.kill('SIGTERM');
        killTimer = setTimeout(() => proc.kill('SIGKILL'), config.killGraceMs);
      };
      controller.signal.addEventListener('abort', abort, { once: true });
      try {
        await new Promise((resolve, reject) => {
          proc.once('error', reject);
          proc.once('close', code => code === 0 ? resolve() : reject(new Error('processing failed')));
        });
        this.check();
        const size = statSync(output).size;
        this.account(size, size);
        // Reaching -fs may mean ffmpeg silently truncated the output.
        if (size >= config.maxFileBytes || bytes >= config.maxScratchBytes) throw new Error('byte limit');
      } finally {
        clearTimeout(killTimer);
        controller.signal.removeEventListener('abort', abort);
      }
    },
  };
}
