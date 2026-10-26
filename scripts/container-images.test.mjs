import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, cpSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { images, metadata, pullImages } from './container-images.mjs';
import { youtubeInputHash, youtubeInputs } from '../backend/scripts/release.mjs';

test('release images exclude the optional custom Bot API build', () => {
  assert.equal(new Set(images.map(image => image.name)).size, images.length);
  for (const image of images) {
    const source = readFileSync(image.file, 'utf8');
    assert.match(source, /^FROM /m);
    if (image.target) assert(source.includes(` AS ${image.target}\n`));
  }
  assert.deepEqual(images.map(image => image.name).sort(), ['backend', 'frontend', 'importer', 'nginx', 'operations', 'youtube-services']);
  const ver = readFileSync('backend/VERSION', 'utf8').trim();
  assert(readFileSync('.env.example', 'utf8').includes(`yoink_rust_image=yoink/rust-backend:${ver}\n`));
});

test('default builds never compile TDLight and the legacy target remains explicit', t => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'yoink-build-contract-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const calls = path.join(root, 'calls');
  for (const tool of ['docker', 'just']) {
    const file = path.join(root, tool);
    writeFileSync(file, `#!/bin/sh\nprintf '%s\\n' '${tool} '"$*" >> "$BUILD_CALLS"\n`);
    chmodSync(file, 0o755);
  }
  const env = { ...process.env, PATH: `${root}:${process.env.PATH}`, BUILD_CALLS: calls };
  const run = target => spawnSync('bash', ['scripts/build.sh', target], { env, encoding: 'utf8', timeout: 30000 });
  const all = run('all');
  assert.equal(all.status, 0, all.stderr);
  const commands = readFileSync(calls, 'utf8');
  assert.match(commands, /release-build/);
  assert.match(commands, /build-youtube-services/);
  assert.match(commands, /Dockerfile.frontend/);
  assert.match(commands, /Dockerfile.nginx/);
  assert.doesNotMatch(commands, /tg-bot-api|tdlight/);
  writeFileSync(calls, '');
  const legacy = run('tg');
  assert.equal(legacy.status, 0, legacy.stderr);
  assert.match(readFileSync(calls, 'utf8'), /Dockerfile.tg-bot-api/);
});

test('development, CI and release images use the same pinned Rust compiler', () => {
  const toolchain = readFileSync('backend/rust-toolchain.toml', 'utf8').match(/channel\s*=\s*"(\d+\.\d+\.\d+)"/)[1];
  const runtime = readFileSync('backend/Dockerfile', 'utf8').match(/^FROM (rust:\S+)/m)[1];
  const probe = readFileSync('backend/Dockerfile.turso-probe', 'utf8').match(/^FROM (rust:\S+)/m)[1];
  assert(runtime.startsWith(`rust:${toolchain}-bookworm@sha256:`));
  assert.equal(probe, runtime);
  assert(readFileSync('.github/workflows/ci.yml', 'utf8').includes(`toolchain: '${toolchain}'`));
});

test('publication normalizes registry names and rejects version/tag mismatches', () => {
  const sha = 'a'.repeat(40);
  assert.deepEqual(metadata('AdamsGH/yoink-core', sha, 'refs/heads/master', '0.1.4'), {
    registry: 'ghcr.io/adamsgh/yoink-core', tag: `sha-${sha}`, version: '0.1.4',
  });
  assert.equal(metadata('AdamsGH/yoink-core', sha, 'refs/tags/v0.1.4', '0.1.4').tag, '0.1.4');
  for (const ref of ['refs/tags/v0.1.3', 'refs/tags/0.1.4', 'refs/tags/v0.1.4;echo unsafe']) {
    assert.throws(() => metadata('AdamsGH/yoink-core', sha, ref, '0.1.4'), /must match/);
  }
  assert.throws(() => metadata('owner/repo/other', sha, 'refs/heads/master', '0.1.4'));
  assert.throws(() => metadata('owner/repo', 'HEAD', 'refs/heads/master', '0.1.4'));
});

test('the example environment parses through the actual just dotenv loader', t => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'yoink-env-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  cpSync('.env.example', path.join(root, '.env'));
  writeFileSync(path.join(root, 'justfile'), 'set dotenv-load := true\ncheck:\n    @test "$backup_cron_schedule" = "0 3 * * *"\n');
  const result = spawnSync('just', ['check'], { cwd: root, encoding: 'utf8', timeout: 30000, env: { PATH: process.env.PATH, HOME: process.env.HOME } });
  assert.equal(result.status, 0, result.stderr);
});

test('image download never starts containers or changes release state', t => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'yoink-pull-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(path.join(root, 'backend'));
  writeFileSync(path.join(root, 'backend/VERSION'), '0.1.4\n');
  writeFileSync(path.join(root, '.env'), 'yoink_youtube_image=yoink/youtube-services:0.1.0\n');
  for (const file of youtubeInputs) {
    mkdirSync(path.dirname(path.join(root, file)), { recursive: true });
    cpSync(file, path.join(root, file));
  }
  const expected = youtubeInputHash(root);
  const calls = [];
  const downloaded = hash => JSON.stringify({ Config: { Labels: { 'io.yoink.youtube-input-sha256': hash } } });
  pullImages(root, args => {
    calls.push(args);
    return args[1] === 'inspect' && args[2].startsWith('ghcr.io/') ? downloaded(expected) : null;
  });
  assert.equal(calls.filter(args => args[0] === 'pull').length, images.length);
  assert(calls.every(args => args[0] === 'pull' || args[0] === 'image' && ['tag', 'inspect'].includes(args[1])));
  assert(!calls.some(args => args.at(-1) === 'yoink/rust-backend:0.1.4'));
  assert(calls.some(args => args.at(-1) === 'yoink/rust-operations:native'));
  assert(calls.some(args => args.at(-1) === 'yoink/youtube-services:0.1.0'));
  const mismatchedCalls = [];
  assert.throws(() => pullImages(root, args => {
    mismatchedCalls.push(args);
    return args[1] === 'inspect' ? downloaded('mismatched-inputs') : null;
  }), /Downloaded YouTube image does not match/);
  assert(!mismatchedCalls.some(args => args[1] === 'tag' && args.at(-1).startsWith('yoink/youtube-services:')));
});
