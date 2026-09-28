import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';

// The caller supplies an already-redacted report. Preserve failures beyond
// fixture cleanup and tools that summarize away the actual assertion.
export async function writeFailureReport(directory, runId, report) {
  if (!/^[a-zA-Z0-9_-]+$/.test(runId)) throw new Error('invalid fixture run ID');
  await mkdir(directory, { recursive: true, mode: 0o700 });
  const destination = path.join(directory, `media-${runId}.log`);
  await writeFile(destination, report, { mode: 0o600, flag: 'wx' });
  return destination;
}
