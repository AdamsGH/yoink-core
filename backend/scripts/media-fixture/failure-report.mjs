import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';

// Bound an already-redacted report without losing the initiating assertion.
export function boundReport(report, maxLength = 24576) {
  if (!Number.isSafeInteger(maxLength) || maxLength < 128) throw new Error('invalid report bound');
  const text = String(report);
  if (text.length <= maxLength) return text;
  const marker = '\n[... diagnostics truncated ...]\n';
  const head = Math.floor((maxLength - marker.length) / 2);
  return text.slice(0, head) + marker + text.slice(-(maxLength - marker.length - head));
}

// The caller supplies an already-redacted report. Preserve failures beyond
// fixture cleanup and tools that summarize away the actual assertion.
export async function writeFailureReport(directory, runId, report) {
  if (!/^[a-zA-Z0-9_-]+$/.test(runId)) throw new Error('invalid fixture run ID');
  await mkdir(directory, { recursive: true, mode: 0o700 });
  const destination = path.join(directory, `media-${runId}.log`);
  await writeFile(destination, report, { mode: 0o600, flag: 'wx' });
  return destination;
}
