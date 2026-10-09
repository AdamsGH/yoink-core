import assert from 'node:assert/strict';

export function configuredImages(env, defaults, sourceMode) {
  const runtime = env.YOINK_MEDIA_RUNTIME_IMAGE;
  const fixture = env.YOINK_MEDIA_FIXTURE_IMAGE;
  if (!runtime && !fixture) return { ...defaults, immutable: false };
  assert(!sourceMode, 'immutable image mode cannot mount native binaries or worker source');
  for (const value of [runtime, fixture]) assert.match(value ?? '', /^sha256:[0-9a-f]{64}$/, 'both images must be immutable local SHA256 image IDs');
  assert.notEqual(runtime, fixture, 'runtime and feature fixture must be separate images');
  return { image: runtime, fixtureImage: fixture, immutable: true };
}

export async function fingerprintImages(docker, images) {
  for (const [role, id] of [['runtime', images.image], ['fixture', images.fixtureImage]]) {
    const inspected = JSON.parse(await docker(['image', 'inspect', id]));
    assert.equal(inspected.length, 1);
    assert.equal(inspected[0].Id, id, 'immutable image fingerprint must match the configured image');
    const entrypoint = inspected[0].Config.Entrypoint;
    assert.deepEqual(entrypoint, [`/usr/local/bin/yoink-${role === 'runtime' ? 'server' : 'fixture-server'}`], 'image target entrypoint must match its role');
    console.log(`Media image fingerprint: ${role}=${id} entrypoint=${entrypoint[0]}`);
  }
}
