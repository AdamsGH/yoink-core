import { closeSync, mkdirSync, openSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

export function summarizeBuild(log, elapsedMs, exitCode) {
  const steps = new Map();
  const compiled = new Set();
  for (const line of log.split('\n')) {
    const header = line.match(/^#(\d+) (\[.+?\] .+|exporting to .+|importing to docker)$/);
    if (header && !steps.has(header[1])) steps.set(header[1], { name: header[2], cached: false, seconds: null });
    const result = line.match(/^#(\d+) (CACHED|DONE(?: ([\d.]+)s)?)$/);
    if (result && steps.has(result[1])) {
      Object.assign(steps.get(result[1]), { cached: result[2] === 'CACHED', seconds: result[3] ? Number(result[3]) : 0 });
    }
    const compilation = line.match(/^#\d+ [\d.]+\s+Compiling (\S+) v/);
    if (compilation) compiled.add(compilation[1]);
  }
  return { elapsedMs, exitCode, cachedSteps: [...steps.values()].filter(s => s.cached).length, steps: [...steps.values()], compiledCrates: [...compiled] };
}

export function reportedBuild(args, { cwd, env, timeout, reportBase }) {
  mkdirSync(path.dirname(reportBase), { recursive: true });
  const logFile = `${reportBase}.log`;
  const fd = openSync(logFile, 'w', 0o600);
  const start = performance.now();
  let result;
  console.log(`Build log: ${logFile}`);
  try {
    result = spawnSync('docker', args, { cwd, env, timeout, stdio: ['ignore', fd, fd] });
  } finally {
    closeSync(fd);
    const report = summarizeBuild(readFileSync(logFile, 'utf8'), Math.round(performance.now() - start), result?.status ?? null);
    report.signal = result?.signal ?? null;
    report.error = result?.error?.message ?? null;
    writeFileSync(`${reportBase}.json`, `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
    console.log(`Build: ${(report.elapsedMs / 1000).toFixed(1)}s, ${report.cachedSteps}/${report.steps.length} cached steps. Report: ${reportBase}.json`);
  }
  return result;
}
