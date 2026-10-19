import { spawn as nodeSpawn } from 'node:child_process';

export function createProcessRunner({ spawn = nodeSpawn, cwd, path = process.env.PATH, children, maxOutput = 24 * 1024 }) {
  function capture(command, args, { timeout = 30000, env = {}, input, allowEarlyStdinClose = false, ...options } = {}) {
    return new Promise((resolve, reject) => {
      const child = spawn(command, args, { cwd, env: { PATH: path, ...env }, stdio: ['pipe', 'pipe', 'pipe'], ...options });
      children.add(child);
      let output = '';
      let settled = false;
      let inputError;
      let processError;
      const timer = setTimeout(() => child.kill('SIGKILL'), timeout);
      const append = chunk => { output = (output + chunk.toString()).slice(-maxOutput); };
      child.stdout.on('data', append);
      child.stderr.on('data', append);
      child.stdin.once('error', error => {
        inputError = error;
        if (!allowEarlyStdinClose || error.code !== 'EPIPE') child.kill('SIGKILL');
      });
      child.once('error', error => { processError = error; });
      child.once('close', (code, signal) => finish(
        processError ?? (allowEarlyStdinClose && inputError?.code === 'EPIPE' ? undefined : inputError),
        { code, signal, output },
      ));
      child.stdin.end(input);
      function finish(error, result) {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        children.delete(child);
        if (error) reject(error);
        else if (result.code !== 0) reject(new Error(`${command} failed (exit ${result.code ?? result.signal}): ${result.output}`));
        else resolve(result);
      }
    });
  }
  return {
    capture,
    async docker(args, options = {}) {
      const result = await capture('docker', args, options);
      return result.output.trim();
    },
  };
}

export function createContainerLifecycle({ docker, capture, containers, assert, runId, maxInput = 1024 * 1024 }) {
  let helperNumber = 0;
  async function startContainer(args) {
    const id = await docker(['run', '--detach', '--pull', 'never', ...args]);
    assert.match(id, /^[0-9a-f]{12,64}$/i, 'docker run must return a container ID');
    containers.add(id);
    return id;
  }
  async function helperContainer(args, timeout = 30000, input, { allowEarlyStdinClose = false } = {}) {
    if (input !== undefined) assert(Buffer.byteLength(input) <= maxInput, 'helper stdin exceeds fixture bound');
    const name = `yoink-media-helper-${runId}-${helperNumber++}`;
    const id = await docker(['create', '--pull', 'never', '--name', name, ...(input === undefined ? [] : ['--interactive']), ...args]);
    assert.match(id, /^[0-9a-f]{12,64}$/i, 'docker create must return a container ID');
    containers.add(id);
    const result = await capture('docker', ['start', '--attach', ...(input === undefined ? [] : ['--interactive']), id], { timeout, input, allowEarlyStdinClose });
    const exitCode = await docker(['inspect', '--format', '{{.State.ExitCode}}', id]);
    assert.equal(exitCode, '0', `fixture container ${name} failed: ${result.output}`);
    return result.output.trim();
  }
  return { startContainer, helperContainer };
}
