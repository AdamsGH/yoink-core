import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { job, config } from '../lifecycle.mjs';
const enabled = process.env.YOINK_CONTAINER_FFMPEG_TESTS === '1';
const children = async () => (await readFile(`/proc/${process.pid}/task/${process.pid}/children`, 'utf8')).trim();
for (const mode of ['deadline', 'disconnect', 'byte']) test(`actual ffmpeg ${mode} terminates and reaps child`, { skip: !enabled }, async () => {
  const before = await children();
  const dir = await mkdtemp(join(tmpdir(), 'ffmpeg-job-'));
  const ctx = job({ ...config(), deadlineMs: mode === 'deadline' ? 200 : 10000, maxFileBytes: mode === 'byte' ? 1000 : 67108864 });
  let timer;
  try {
    const output = join(dir, 'out.m4a');
    const promise = ctx.ffmpeg(['-y', '-re', '-f', 'lavfi', '-i', 'sine=frequency=440', '-c:a', 'aac', output], output);
    if (mode === 'disconnect') timer = setTimeout(() => ctx.abort(), 200);
    await assert.rejects(promise);
    assert.equal(await children(), before, 'ffmpeg child must be reaped before releasing slot');
    if (mode === 'byte') assert.ok((await stat(output)).size < 10000);
  } finally { clearTimeout(timer); ctx.dispose(); await rm(dir, { recursive: true, force: true }); }
});
