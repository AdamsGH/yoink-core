import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { cpSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { images, metadata, pullImages } from './container-images.mjs';
import { youtubeInputHash, youtubeInputs } from '../backend/scripts/release.mjs';

test('every deployable image has a checked-in Dockerfile and unique package', () => {
  assert.equal(new Set(images.map(image => image.name)).size, images.length);
  for (const image of images) {
    const source = readFileSync(image.file, 'utf8');
    assert.match(source, /^FROM /m);
    if (image.target) assert(source.includes(` AS ${image.target}\n`));
  }
  assert.deepEqual(images.map(image => image.name).sort(), ['backend', 'frontend', 'importer', 'nginx', 'operations', 'tg-bot-api', 'youtube-services']);
  const ver = readFileSync('backend/VERSION', 'utf8').trim();
  assert(readFileSync('.env.example', 'utf8').includes(`yoink_rust_image=yoink/rust-backend:${ver}\n`));
  assert.match(readFileSync('scripts/build.sh', 'utf8'), /all\)[\s\S]*run_build tg/);
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
