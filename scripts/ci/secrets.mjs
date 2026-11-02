import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { realpathSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const SCANNER_IMAGE = 'zricethezav/gitleaks:v8.30.1@sha256:c00b6bd0aeb3071cbcb79009cb16a60dd9e0a7c60e2be9ab65d25e6bc8abbb7f';
export const REPOSITORIES = ['.', 'plugins/yoink-dl', 'plugins/yoink-insight', 'plugins/yoink-music', 'plugins/yoink-stats'];
const project = fileURLToPath(new URL('../../', import.meta.url));

export function scannerArgs(metadata, config = path.join(project, '.gitleaks.toml'), ignore = path.join(project, '.gitleaksignore')) {
  const relative = path.relative(metadata.common, metadata.directory);
  assert(!relative.startsWith('..') && !path.isAbsolute(relative), 'Git directory must be inside the common metadata directory.');
  return ['run', '--rm', '--network', 'none', '--read-only', '--cap-drop', 'ALL',
    '--user', `${process.getuid()}:${process.getgid()}`,
    '--tmpfs', '/repo:ro,noexec,nosuid,nodev,size=1m,mode=0755',
    '--mount', `type=bind,src=${metadata.common},dst=/git-metadata,readonly`,
    '--mount', `type=bind,src=${config},dst=/gitleaks.toml,readonly`,
    '--mount', `type=bind,src=${ignore},dst=/.gitleaksignore,readonly`,
    '--env', 'GIT_WORK_TREE=/repo', '--env', `GIT_DIR=${path.posix.join('/git-metadata', relative)}`,
    SCANNER_IMAGE, 'git', '/repo', '--log-opts=--all', '--redact=100', '--no-banner', '--no-color', '--log-level=info',
    '--config=/gitleaks.toml', '--gitleaks-ignore-path=/.gitleaksignore', '--report-format=json', '--report-path=-'];
}

function repositoryMetadata(repository) {
  const result = spawnSync('git', ['-C', repository, 'rev-parse', '--absolute-git-dir', '--path-format=absolute', '--git-common-dir', '--is-shallow-repository', '--show-toplevel'], { encoding: 'utf8', timeout: 10000 });
  assert(!result.error && result.status === 0, 'Secret scanner could not resolve repository metadata.');
  const [directory, common, shallow, top] = result.stdout.trim().split('\n');
  assert(directory && common, 'Secret scanner received incomplete repository metadata.');
  assert(top && realpathSync(top) === realpathSync(repository), 'Secret scanner target is not a repository root. Initialize the submodule before scanning.');
  assert(shallow === 'false', 'Full Git history is required for secret scanning. Fetch the complete repository history.');
  return { directory, common };
}

export function scanRepository(repository, config, ignore) {
  // History scanning needs Git metadata, not runtime files in the working tree.
  const result = spawnSync('docker', scannerArgs(repositoryMetadata(repository), config, ignore), {
    encoding: 'utf8', timeout: 120000, maxBuffer: 16 * 1024 ** 2,
  });
  assert(!result.error, `Secret scanner could not run: ${result.error?.message}`);
  assert([0, 1].includes(result.status), `Secret scanner failed (${result.status}).`);
  // Gitleaks can exit successfully after Git errors without scanning any commits.
  assert(!/\bERR\b/.test(result.stderr) && /\b[1-9]\d* commits scanned\./.test(result.stderr), 'Secret scanner could not scan repository history.');
  let findings;
  try {
    findings = JSON.parse(result.stdout);
  } catch {
    throw new Error('Secret scanner did not produce a JSON report.');
  }
  assert(Array.isArray(findings) && ((result.status === 0) === (findings.length === 0)), 'Secret scanner status/report mismatch.');
  return findings;
}

function main() {
  let count = 0;
  for (const repository of REPOSITORIES) {
    const findings = scanRepository(path.join(project, repository));
    // Report locations and rules only. Never print matched text or credentials.
    for (const finding of findings) console.error(JSON.stringify({ repository, file: finding.File, line: finding.StartLine, rule: finding.RuleID }));
    count += findings.length;
    console.log(`Secret history scan ${repository}: ${findings.length} findings.`);
  }
  if (count) process.exitCode = 1;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { main(); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
