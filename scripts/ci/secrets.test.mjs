import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { REPOSITORIES, scannerArgs, scanRepository } from './secrets.mjs';

// Construct the fixture at runtime so it is not a finding in its own test source.
const coverage = ['retry', 'cancellation', 'quota'].join('/');
const prose = `The fixture covers access changes, ${coverage} behavior.\n`;
const media = 'docs/development/media-fixtures.md';
const project = fileURLToPath(new URL('../../', import.meta.url));

function syntheticKey(length = 48) {
  // Random hex can hit Gitleaks entropy limits or its "dead"/"feed" stopwords.
  const alphabet = '0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz';
  return Array.from({ length }, (_, index) => alphabet[(index * 19 + 7) % alphabet.length]).join('');
}

function fixture(t) {
  // The isolated runner and its sibling Docker daemon share the checkout, not /tmp.
  const target = path.join(project, 'backend/target');
  mkdirSync(target, { recursive: true });
  const directory = mkdtempSync(path.join(target, 'secrets-test-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const git = (...args) => {
    const result = spawnSync('git', ['-c', 'core.hooksPath=/dev/null', '-c', 'commit.gpgSign=false', ...args], {
      cwd: directory, encoding: 'utf8', env: { ...process.env, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1' },
    });
    assert.equal(result.status, 0, result.stderr);
  };
  git('init', '--quiet', '--initial-branch=master');
  git('config', 'user.name', 'CI Fixture');
  git('config', 'user.email', 'fixture@example.invalid');
  const commit = (file, content) => {
    mkdirSync(path.dirname(path.join(directory, file)), { recursive: true });
    writeFileSync(path.join(directory, file), content);
    git('add', '--all');
    git('commit', '--quiet', '-m', 'Scanner regression fixture');
  };
  return { directory, git, commit };
}

test('scanner retains full history, redaction, shared policy, and disposable Docker execution', () => {
  const args = scannerArgs({ directory: '/fixture/.git', common: '/fixture/.git' }, '/policy', '/ignore');
  for (const arg of ['--rm', '--read-only', '--log-opts=--all', '--redact=100', '--config=/gitleaks.toml']) assert(args.includes(arg));
  assert(args.includes('type=bind,src=/policy,dst=/gitleaks.toml,readonly'));
  assert.deepEqual(args.filter(arg => arg.startsWith('type=bind,')), [
    'type=bind,src=/fixture/.git,dst=/git-metadata,readonly',
    'type=bind,src=/policy,dst=/gitleaks.toml,readonly',
    'type=bind,src=/ignore,dst=/.gitleaksignore,readonly',
  ]);
  assert.equal(args[args.indexOf('--network') + 1], 'none');
  const modules = spawnSync('git', ['config', '--file', path.join(project, '.gitmodules'), '--get-regexp', '^submodule\..*\.path$'], { encoding: 'utf8' });
  assert.equal(modules.status, 0, modules.stderr);
  const paths = modules.stdout.trim().split('\n').map(line => line.split(' ')[1]);
  assert.deepEqual([...REPOSITORIES.slice(1)].sort(), paths.sort());
});

test('only the reviewed media-coverage phrase is allowed', t => {
  const f = fixture(t);
  f.commit(media, prose);
  assert.deepEqual(scanRepository(f.directory), []);
});

test('empty history does not count as a successful scan', t => {
  const f = fixture(t);
  assert.throws(() => scanRepository(f.directory), /could not scan repository history/);
});

test('submodule scans resolve external gitdirs without mounting the parent checkout', t => {
  const parent = fixture(t);
  const plugin = fixture(t);
  parent.commit('README.md', '# Parent\n');
  plugin.commit('credentials.txt', `api_key = "${syntheticKey()}"\n`);
  parent.git('-c', 'protocol.file.allow=always', 'submodule', 'add', '--quiet', plugin.directory, 'plugins/fixture');
  parent.git('commit', '--quiet', '-am', 'Add fixture submodule');
  const child = path.join(parent.directory, 'plugins/fixture');
  assert(scanRepository(child).some(finding => finding.RuleID === 'generic-api-key'));
});

test('linked worktrees use their external shared metadata without skipping history', t => {
  const f = fixture(t);
  f.commit('credentials.txt', `api_key = "${syntheticKey()}"\n`);
  const worktree = path.join(f.directory, 'linked');
  f.git('worktree', 'add', '--quiet', '-b', 'linked', worktree);
  assert(scanRepository(worktree).some(finding => finding.RuleID === 'generic-api-key'));
});

test('shallow clones cannot pass as a full-history scan', t => {
  const f = fixture(t);
  f.commit('README.md', '# Fixture\n');
  const shallow = path.join(f.directory, 'shallow');
  f.git('-c', 'protocol.file.allow=always', 'clone', '--quiet', '--depth=1', `file://${f.directory}`, shallow);
  assert.throws(() => scanRepository(shallow), /Full Git history is required/);
});

test('an uninitialized submodule cannot silently scan the parent history instead', t => {
  const f = fixture(t);
  f.commit('README.md', '# Fixture\n');
  const missing = path.join(f.directory, 'plugins/missing');
  mkdirSync(missing, { recursive: true });
  assert.throws(() => scanRepository(missing), /not a repository root/);
});

test('the phrase is not exempt in other files', t => {
  const f = fixture(t);
  f.commit('credentials.txt', prose);
  assert(scanRepository(f.directory).some(finding => finding.RuleID === 'generic-api-key'));
});

test('generic API credentials in the same documentation file still fail', t => {
  const f = fixture(t);
  const secret = syntheticKey();
  f.commit(media, prose + `api_key = "${secret}"\n`);
  const findings = scanRepository(f.directory);
  assert(findings.some(finding => finding.RuleID === 'generic-api-key'));
  assert(!JSON.stringify(findings).includes(secret));
});

test('provider-specific default rules still detect credentials', t => {
  const f = fixture(t);
  f.commit(media, `token = "${'ghp_'}${syntheticKey(36)}"\n`);
  assert(scanRepository(f.directory).some(finding => finding.RuleID === 'github-pat'));
});

test('deleted credentials remain detectable in history and non-current branches', t => {
  const f = fixture(t);
  f.commit('README.md', '# Fixture\n');
  f.git('checkout', '--quiet', '-b', 'old-credential');
  f.commit('credentials.txt', `api_key = "${syntheticKey()}"\n`);
  f.commit('credentials.txt', 'Removed\n');
  f.git('checkout', '--quiet', 'master');
  assert(scanRepository(f.directory).some(finding => finding.RuleID === 'generic-api-key'));
});
