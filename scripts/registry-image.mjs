import { appendFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export async function publishedImage({ image, tag, revision, user, token }, request = fetch) {
  if (!/^ghcr\.io\/[a-z0-9_.-]+\/[a-z0-9_./-]+$/.test(image) || !/^[a-zA-Z0-9_.-]+$/.test(tag) || !/^[a-f0-9]{40}$/.test(revision)) {
    throw new Error('Invalid immutable image reference.');
  }
  const repository = image.slice('ghcr.io/'.length);
  const auth = await request(`https://ghcr.io/token?service=ghcr.io&scope=repository:${repository}:pull`, {
    headers: { Authorization: `Basic ${Buffer.from(`${user}:${token}`).toString('base64')}` }, signal: AbortSignal.timeout(30000),
  });
  if (!auth.ok) throw new Error(`GHCR authorization failed (${auth.status}).`);
  const access = (await auth.json()).token;
  if (!access) throw new Error('GHCR did not return a read token.');
  const headers = { Authorization: `Bearer ${access}`, Accept: 'application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json' };
  const get = resource => request(`https://ghcr.io/v2/${repository}/${resource}`, { headers, signal: AbortSignal.timeout(30000) });
  let manifest = await get(`manifests/${tag}`);
  if (manifest.status === 404) return false;
  if (!manifest.ok) throw new Error(`GHCR manifest check failed (${manifest.status}).`);
  let data = await manifest.json();
  if (data.manifests) {
    const platform = data.manifests.find(entry => entry.platform?.os === 'linux' && entry.platform?.architecture === 'amd64');
    if (!platform || !/^sha256:[a-f0-9]{64}$/.test(platform.digest)) throw new Error('Published tag has no linux/amd64 image.');
    manifest = await get(`manifests/${platform.digest}`);
    if (!manifest.ok) throw new Error(`GHCR platform check failed (${manifest.status}).`);
    data = await manifest.json();
  }
  if (!/^sha256:[a-f0-9]{64}$/.test(data.config?.digest)) throw new Error('Invalid published image configuration.');
  const config = await get(`blobs/${data.config.digest}`);
  if (!config.ok) throw new Error(`GHCR image configuration check failed (${config.status}).`);
  const labels = (await config.json()).config?.Labels;
  if (labels?.['org.opencontainers.image.revision'] !== revision) {
    throw new Error('Published tag belongs to another commit. Immutable tags cannot be replaced.');
  }
  return true;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const exists = await publishedImage({ image: process.env.GHCR_IMAGE, tag: process.env.GHCR_TAG, revision: process.env.GITHUB_SHA, user: process.env.GHCR_USER, token: process.env.GHCR_TOKEN });
  appendFileSync(process.env.GITHUB_OUTPUT, `exists=${exists}\n`);
  console.log(exists ? 'Matching immutable image already published. Skipping rebuild.' : 'Image tag is available for publication.');
}
