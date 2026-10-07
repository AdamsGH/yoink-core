// Test-only entry point: no SDK session/provider networking. Still uses patched routes,
// lifecycle, real ffmpeg merge and HTTP streaming. Never use in operator deployment.
import { createApp } from '../index.js';
import { config, job } from '../lifecycle.mjs';
import { mkdir, readFile } from 'node:fs/promises';
const settings = config();
const dir = '/tmp/synthetic'; await mkdir(dir, { recursive: true });
const ctx = job({ ...settings, deadlineMs: 10000 });
try {
  await ctx.ffmpeg(['-y', '-f', 'lavfi', '-i', 'color=c=blue:s=160x90:r=10:d=1', '-an', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', dir + '/video.mp4'], dir + '/video.mp4');
  await ctx.ffmpeg(['-y', '-f', 'lavfi', '-i', 'sine=frequency=440:duration=1', '-c:a', 'aac', dir + '/audio.m4a'], dir + '/audio.m4a');
} finally { ctx.dispose(); }
const video = await readFile(dir + '/video.mp4'), audio = await readFile(dir + '/audio.m4a');
createApp({ settings, session: async (tokens, update) => {
  if (tokens.access_token === 'synthetic-refresh-observed') update(tokens);
  return { getBasicInfo: async () => ({ basic_info: { title: 'Synthetic topology', duration: 1, author: 'Fixture' }, download: async options => new ReadableStream({ start(c) { c.enqueue(options.type === 'audio' ? audio : video); c.close(); } }) }) };
} }).listen(settings.port);
