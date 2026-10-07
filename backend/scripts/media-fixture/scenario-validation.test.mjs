import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const driver = fileURLToPath(new URL('../test-media.mjs', import.meta.url));

for (const args of [['unsupported-native-fixture'], ['all', 'unexpected-extra-argument']]) {
  test(`native fixture rejects invalid arguments: ${args.join(' ')}`, () => {
    const result = spawnSync(process.execPath, [driver, ...args], {
      env: { ...process.env, YOINK_MEDIA_PROVIDER_SOURCE_MOUNT: '1' },
      encoding: 'utf8',
      timeout: 5_000,
    });
    assert.ifError(result.error);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /usage: test-media\.mjs/);
    assert.equal(result.stdout, '');
  });
}
