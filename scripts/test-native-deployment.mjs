import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, copyFileSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';

test('default and rollback Compose select exclusive runtimes and confined native files', () => {
  const root = mkdtempSync(join(tmpdir(), 'yoink-deployment-'));
  try {
    for (const file of ['docker-compose.yaml', 'docker-compose.legacy.yaml']) copyFileSync(file, join(root, file));
    writeFileSync(join(root, '.env'), 'yoink_rust_image=yoink/rust-backend:0.1.0\nbot_token=123:fixture\nowner_id=42\napi_secret_key=fixture-secret\nyoink_domain=fixture.invalid\napi_id=123\napi_hash=fixture\n');
    const read = legacy => {
      const args = ['compose', '--project-directory', root, '-f', join(root, 'docker-compose.yaml')];
      if (legacy) args.push('-f', join(root, 'docker-compose.legacy.yaml'));
      args.push('config', '--format', 'json');
      const result = spawnSync('docker', args, { encoding: 'utf8', timeout: 30000, maxBuffer: 2 * 1024 * 1024, env: { PATH: process.env.PATH, HOME: process.env.HOME } });
      assert.equal(result.status, 0, 'isolated Compose configuration must parse');
      return JSON.parse(result.stdout);
    };
    const native = read(false).services;
    assert.equal(native.yoink.image, 'yoink/rust-backend:0.1.0');
    assert.equal(native.yoink.build, undefined);
    assert.equal(native.yoink.user, '10001:10001');
    assert.equal(native.yoink.environment.YOINK_RUST_BOT_API_FILE_ROOT, '/var/lib/telegram-bot-api');
    assert.equal(native.yoink.environment.YOINK_RUST_TELEGRAM_LOCAL_FILE_ROOT, '/telegram-files');
    assert(native.yoink.volumes.some(v => v.target === '/telegram-files' && v.read_only));
    assert(!native.yoink.volumes.some(v => v.target.startsWith('/app')));
    assert.equal(native['yoink-arq-worker'], undefined);
    for (const service of ['yoink-postgres', 'yoink-redis', 'yoink-youtubei', 'bgutil', 'native-youtube-auth-init']) assert.equal(native[service], undefined);
    assert.deepEqual(Object.keys(native).sort(), ['native-youtube-auth-gateway', 'tg-bot-api', 'yoink', 'yoink-frontend', 'yoink-nginx', 'yoink-proxy']);
    const gateway = native['native-youtube-auth-gateway'];
    assert.deepEqual(Object.keys(gateway.depends_on), ['yoink-proxy']);
    assert.equal(gateway.image, 'yoink/youtube-services:0.1.0');
    assert.equal(gateway.build, undefined);
    assert.equal(gateway.user, '10001:10001');
    assert.equal(gateway.init, true);
    assert.equal(gateway.read_only, true);
    assert.equal(gateway.pids_limit, 192);
    assert.equal(Number(gateway.mem_limit), 1610612736);
    assert.equal(gateway.ports, undefined);
    const rollback = read(true).services;
    assert.equal(rollback.yoink.image, 'yoink/yoink:latest');
    assert.equal(rollback.yoink.build.dockerfile, 'docker/Dockerfile');
    assert.equal(rollback.yoink.build.target, undefined);
    assert(!rollback.yoink.volumes.some(v => v.target === '/state'));
    assert(!rollback['yoink-arq-worker'].profiles?.length);
    for (const service of ['yoink-postgres', 'yoink-redis', 'bgutil', 'yoink-youtubei']) assert(rollback[service]);
    assert.equal(rollback['native-youtube-auth-gateway'], undefined);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('legacy migration dispatcher cannot accidentally start a native poller', () => {
  const result = spawnSync('bash', ['scripts/migrate.sh', 'up'], { encoding: 'utf8', timeout: 3000 });
  assert.equal(result.status, 2);
  assert.match(result.stderr, /Legacy Alembic execution is disabled/);
});

test('native restore rejects traversal before invoking Docker', () => {
  const result = spawnSync(process.execPath, ['scripts/native-backup.mjs', 'restore', '../foreign.ybk'], { encoding: 'utf8', timeout: 3000, env: { ...process.env, PATH: '' } });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Existing state and archives were not overwritten/);
  assert.equal(result.stdout, '');
});

test('native maintenance resumes only the original container after success or archive failure', () => {
  const directory = mkdtempSync(join(tmpdir(), 'yoink-maintenance-'));
  try {
    const log = join(directory, 'calls.jsonl');
    const executable = join(directory, 'docker');
    writeFileSync(executable, `#!${process.execPath}
const fs = require('node:fs');
const args = process.argv.slice(2);
fs.appendFileSync(process.env.CALL_LOG, JSON.stringify(args) + '\\n');
if (args[0] === 'inspect') console.log(JSON.stringify([{Id: 'original-container', Config: {Entrypoint: ['/usr/local/bin/yoink-server'], Env: ['YOINK_RUST_DATABASE_PATH=/state/native.db']}, Mounts: [{Destination: '/state', Source: process.cwd() + '/data/native'}], State: {Running: args[1] === 'yoink', ExitCode: 0, OOMKilled: false}}]));
if (args[0] === 'image') console.log('sha256:' + '0'.repeat(64));
if (args[0] === 'run' && process.env.FAIL_ARCHIVE === '1') process.exit(1);
if (args[0] === 'start' && process.env.FAIL_START === '1') process.exit(1);
`, { mode: 0o700 });
    for (const [failure, missing, expected] of [['0', '0', 0], ['1', '0', 1], ['0', '1', 1]]) {
      writeFileSync(log, '');
      const result = spawnSync(process.execPath, ['scripts/native-backup.mjs', 'run'], { encoding: 'utf8', timeout: 5000, env: { ...process.env, PATH: directory, CALL_LOG: log, FAIL_ARCHIVE: failure, FAIL_START: missing } });
      assert.equal(result.status, expected);
      const calls = readFileSync(log, 'utf8').trim().split('\n').map(line => JSON.parse(line));
      assert.deepEqual(calls.find(args => args[0] === 'stop'), ['stop', '--time', '165', 'original-container']);
      assert.deepEqual(calls.at(-1), ['start', 'original-container']);
      assert(!calls.some(args => args[0] === 'compose'));
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
