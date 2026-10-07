import assert from 'node:assert/strict';
import { test } from 'node:test';
import { verifyNativeFailure } from './native-rejection.mjs';

const exited = { Status: 'exited', Running: false, OOMKilled: false, Error: '', StartedAt: '2026-01-01T00:00:00Z', ExitCode: 1 };
const reason = /^invalid importer configuration\s*$/;

test('rejection requires actual native execution and its specific reason', () => {
  verifyNativeFailure(exited, 'invalid importer configuration\n', reason);
  for (const change of [
    { Status: 'running' }, { Running: true }, { OOMKilled: true },
    { Error: 'executable not found' }, { StartedAt: '0001-01-01T00:00:00Z' },
    { ExitCode: 0 }, { ExitCode: 127 }, { ExitCode: 137 },
  ]) {
    assert.throws(() => verifyNativeFailure({ ...exited, ...change }, 'invalid importer configuration', reason));
  }
  for (const message of ['docker failed (exit 1)', 'source connection failed', 'invalid importer configuration; unrelated failure']) {
    assert.throws(() => verifyNativeFailure(exited, message, reason));
  }
});
