import { appendFileSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { execute, inputHash, version, youtubeInputHash, youtubeImage } from '../backend/scripts/release.mjs';

export const images = [
  { name: 'backend', context: 'backend', file: 'backend/Dockerfile', target: 'runtime' },
  { name: 'operations', context: 'backend', file: 'backend/Dockerfile', target: 'operations' },
  { name: 'importer', context: 'backend', file: 'backend/Dockerfile', target: 'importer' },
  { name: 'frontend', context: '.', file: 'docker/Dockerfile.frontend', target: '' },
  { name: 'nginx', context: '.', file: 'docker/Dockerfile.nginx', target: '' },
  { name: 'youtube-services', context: '.', file: 'backend/Dockerfile.youtube-auth-gateway', target: '' },
];

export function metadata(repository, sha, ref, releaseVersion) {
  version(releaseVersion);
  if (!/^[a-zA-Z0-9_.-]+\/[a-zA-Z0-9_.-]+$/.test(repository) || !/^[a-f0-9]{40}$/.test(sha)) throw new Error('Invalid repository or commit SHA.');
  if (ref.startsWith('refs/tags/') && ref !== `refs/tags/v${releaseVersion}`) throw new Error('Release tag must match backend/VERSION.');
  return { registry: `ghcr.io/${repository.toLowerCase()}`, tag: ref.startsWith('refs/tags/') ? releaseVersion : `sha-${sha}`, version: releaseVersion };
}

export function pullImages(root, run = (args, options) => execute(args, { cwd: root, ...options })) {
  const ver = version(readFileSync(path.join(root, 'backend/VERSION'), 'utf8').trim());
  const registry = 'ghcr.io/adamsgh/yoink-core';
  for (const image of images) run(['pull', `${registry}/${image.name}:${ver}`], { live: true });
  for (const image of images) {
    const remote = `${registry}/${image.name}:${ver}`;
    // The release manager owns immutable runtime aliases. It validates the pulled labels before adoption.
    if (image.name === 'backend') continue;
    const local = image.name === 'operations' ? 'yoink/rust-operations:native'
      : image.name === 'importer' ? 'yoink/rust-importer:experimental'
      : image.name === 'youtube-services' ? youtubeImage(root) : `yoink/${image.name}:latest`;
    if (image.name === 'youtube-services') {
      const expected = youtubeInputHash(root);
      const downloaded = JSON.parse(run(['image', 'inspect', remote, '--format', '{{json .}}']));
      if (downloaded.Config?.Labels?.['io.yoink.youtube-input-sha256'] !== expected) {
        throw new Error('Downloaded YouTube image does not match this checkout. No YouTube alias changed.');
      }
      const existing = run(['image', 'inspect', local, '--format', '{{json .}}'], { optional: true });
      if (existing !== null) {
        if (JSON.parse(existing).Config?.Labels?.['io.yoink.youtube-input-sha256'] !== expected) {
          throw new Error('Local YouTube tag has different inputs. Choose a new yoink_youtube_image version before downloading.');
        }
        continue;
      }
    }
    run(['image', 'tag', remote, local]);
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = fileURLToPath(new URL('../', import.meta.url));
  const ver = readFileSync(path.join(root, 'backend/VERSION'), 'utf8').trim();
  if (process.argv[2] === 'metadata') {
    const data = metadata(process.env.GITHUB_REPOSITORY, process.env.GITHUB_SHA, process.env.GITHUB_REF, ver);
    data.matrix = JSON.stringify({ include: images });
    data.runtime_hash = inputHash(path.join(root, 'backend'));
    data.youtube_hash = youtubeInputHash(root);
    for (const [name, value] of Object.entries(data)) appendFileSync(process.env.GITHUB_OUTPUT, `${name}=${value}\n`);
  } else if (process.argv[2] === 'pull') {
    pullImages(root);
    console.log('Images downloaded. No containers started or release state changed.');
  } else throw new Error('Usage: container-images.mjs metadata | pull');
}
