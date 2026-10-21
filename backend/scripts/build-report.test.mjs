import assert from 'node:assert/strict';
import { test } from 'node:test';
import { summarizeBuild } from './build-report.mjs';

test('reports interleaved steps, cache hits and actual compiled crates without summing parallel durations', () => {
  const report = summarizeBuild(`#1 [rust-build 1/1] RUN cargo build
#2 [worker-runtime 2/4] RUN pip install
#2 CACHED
#1 ...
#1 [rust-build 1/1] RUN cargo build
#1 0.5    Compiling yoink-telegram v0.1.0 (/build)
#1 0.6    Compiling yoink-telegram v0.1.0 (/build)
#1 DONE 10.3s
#3 exporting to oci image format
#3 DONE 2.1s
`, 11000, 0);
  assert.equal(report.elapsedMs, 11000);
  assert.equal(report.cachedSteps, 1);
  assert.equal(report.steps.length, 3);
  assert.equal(report.steps[0].seconds, 10.3);
  assert.deepEqual(report.compiledCrates, ['yoink-telegram']);
});

test('failed or interrupted steps remain incomplete, not cache hits or successful zero-duration steps', () => {
  const report = summarizeBuild('#1 [rust-build 1/1] RUN cargo build\n#1 ERROR: cancelled\n', 1200, null);
  assert.equal(report.exitCode, null);
  assert.equal(report.cachedSteps, 0);
  assert.equal(report.steps[0].seconds, null);
});
