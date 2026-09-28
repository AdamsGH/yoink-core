import { spawn } from 'node:child_process';

const LIMIT = 1024 * 1024;

export function start(binary, args, { cwd, env = process.env, input, keepStdin = false } = {}) {
  const child = spawn(binary, args, { cwd, env, stdio: ['pipe', 'pipe', 'pipe'], shell: false });
  const output = { stdout: [], stderr: [] };
  let bytes = 0;
  let overflow = false;
  let inputError;
  let spawnError;
  let escalation;
  child.once('error', error => { spawnError = error; });
  child.stdin.on('error', error => { inputError = error; });
  for (const label of ['stdout', 'stderr']) {
    child[label].on('data', chunk => {
      const available = Math.max(0, LIMIT - bytes);
      const retained = chunk.subarray(0, available);
      if (retained.length) output[label].push(retained);
      bytes += retained.length;
      if (chunk.length > available && !overflow) {
        overflow = true;
        child.kill('SIGTERM');
        escalation = setTimeout(() => child.kill('SIGKILL'), 1500);
      }
    });
  }
  const closed = new Promise(resolve => child.once('close', (code, signal) => {
    clearTimeout(escalation);
    resolve({ code, signal });
  }));
  if (keepStdin) {
    if (input) child.stdin.write(input);
  } else child.stdin.end(input ?? '');
  return {
    child, stdin: child.stdin, closed,
    get stdout() { return Buffer.concat(output.stdout).toString('utf8'); },
    get stderr() { return Buffer.concat(output.stderr).toString('utf8'); },
    get overflow() { return overflow; },
    get inputError() { return inputError; },
    get spawnError() { return spawnError; },
    kill: signal => child.kill(signal),
    async terminate(graceMs = 1500) {
      if (child.exitCode !== null || child.signalCode !== null || spawnError) return closed;
      child.kill('SIGTERM');
      let timer;
      try {
        const result = await Promise.race([closed, new Promise(resolve => {
          timer = setTimeout(() => resolve(null), graceMs);
        })]);
        if (result !== null) return result;
        child.kill('SIGKILL');
        return await closed;
      } finally { clearTimeout(timer); }
    },
  };
}

export async function run(binary, args, { cwd, env = process.env, input, timeout = 60_000 } = {}) {
  let proc;
  try { proc = start(binary, args, { cwd, env, input }); }
  catch { throw failure('spawn', 'could not start child process'); }
  let timer;
  try {
    const result = await Promise.race([proc.closed, new Promise(resolve => {
      timer = setTimeout(() => resolve(null), timeout);
    })]);
    if (result === null) {
      await proc.terminate();
      throw failure('timeout', `timed out after ${timeout}ms`, proc);
    }
    if (proc.spawnError) throw failure('spawn', 'could not start child process', proc);
    if (proc.overflow) throw failure('output_limit', `output exceeded ${LIMIT} bytes`, proc);
    if (proc.inputError) throw failure('stdin', 'failed to write child stdin', proc);
    if (result.code !== 0) throw failure('exit', `exited ${result.code ?? result.signal ?? 'without status'}`, proc);
    return { code: result.code, stdout: proc.stdout, stderr: proc.stderr };
  } finally { clearTimeout(timer); }
}

function failure(category, message, proc) {
  const error = new Error(`child ${category}: ${message}`);
  error.category = category;
  error.stdout = proc?.stdout ?? '';
  error.stderr = proc?.stderr ?? '';
  return error;
}

export function assertNoSecrets(output, secrets) {
  for (const secret of secrets.filter(Boolean)) {
    if (output.includes(secret)) throw new Error('command output exposed fixture credentials');
  }
}
