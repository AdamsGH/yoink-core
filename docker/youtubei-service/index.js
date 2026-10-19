import express from 'express';
import { Innertube, UniversalCache, Platform, Log } from 'youtubei.js';
import { mkdirSync, rmSync, existsSync } from 'fs';
import { join } from 'path';
import { randomBytes } from 'crypto';
import { config, job } from './lifecycle.mjs';
import vm from 'node:vm';
import { pathToFileURL } from 'node:url';

Log.setLevel(Log.Level.NONE);

// v17+: provide JS evaluator for deciphering streaming URLs
Platform.shim.eval = async (data, env) => vm.runInNewContext(`(function() { ${data.output}\n })()`, { ...env }, { timeout: config().evalTimeoutMs });

async function makeInnertube(tokens, onUpdate = () => {}, ctx) {
  const yt = await Innertube.create({
    cache: new UniversalCache(false),
    fetch: ctx.fetch,
    generate_session_locally: true,
    client_type: 'TVHTML5',
    user_agent: 'Mozilla/5.0 (ChromiumStylePlatform) Cobalt/Version',
  });

  yt.session.on('update-credentials', ({ credentials }) => onUpdate(credentials));
  await ctx.run(new Promise((resolve, reject) => {
    yt.session.once('auth', resolve);
    yt.session.on('auth-error', (err) => reject(err));
    yt.session.signIn(tokens).catch(reject);
  }));

  return yt;
}

function buildClipArgs(startSec, endSec) {
  // Use -ss before inputs for fast keyframe seek, then -t for duration.
  // -to after -i would be absolute time from file start, not from -ss.
  const pre = [];
  const post = [];
  if (startSec != null) { pre.push('-ss', String(startSec)); }
  if (endSec != null) {
    const duration = startSec != null ? endSec - startSec : endSec;
    post.push('-t', String(duration));
  }
  return { pre, post };
}

function ffmpegMerge(ctx, videoPath, audioPath, outputPath, startSec, endSec) {
  const { pre, post } = buildClipArgs(startSec, endSec);
  const args = ['-y', ...pre, '-i', videoPath, '-i', audioPath, '-c:v', 'copy', '-c:a', 'copy', ...post, outputPath];
  return ctx.ffmpeg(args, outputPath);
}

function ffmpegTrim(ctx, inputPath, outputPath, startSec, endSec) {
  const { pre, post } = buildClipArgs(startSec, endSec);
  const args = ['-y', ...pre, '-i', inputPath, '-c', 'copy', ...post, outputPath];
  return ctx.ffmpeg(args, outputPath);
}

// Map user quality setting to youtubei.js quality label
function resolveQuality(quality) {
  const map = {
    'best': 'best',
    'bestefficiency': 'bestefficiency',
    '2160': '2160p',
    '1440': '1440p',
    '1080': '1080p',
    '720': '720p',
    '480': '480p',
    '360': '360p',
    '240': '240p',
    '144': '144p',
  };
  return map[quality] || quality;
}

export function createApp({ settings = config(), session = makeInnertube, downloadDir = process.env.DOWNLOAD_DIR || '/tmp/youtubei' } = {}) {
const app = express();
const DOWNLOAD_DIR = downloadDir;
if (!existsSync(DOWNLOAD_DIR)) mkdirSync(DOWNLOAD_DIR, { recursive: true, mode: 0o700 });
let active = 0;
app.use((req, res, next) => {
  if (req.path === '/health') return next();
  if (active >= settings.concurrency) return res.status(503).json({ error: 'service busy' });
  active++;
  const ctx = job(settings);
  res.locals.ctx = ctx;
  const disconnected = () => ctx.abort();
  res.once('close', disconnected);
  // Keep the slot until handler/child cleanup completes, not merely socket close.
  let released = false;
  res.locals.release = () => { if (released) return; released = true; ctx.dispose(); res.removeListener('close', disconnected); active--; };
  const abortParsing = () => { if (!res.locals.started) { res.locals.release(); req.destroy(); } };
  ctx.signal.addEventListener('abort', abortParsing, { once: true });
  next();
});
app.use(express.json({ limit: '64kb' }));
app.use((req, res, next) => { res.locals.started = true; next(); });
// POST /download
// Body: { url, tokens: { access_token, refresh_token, expiry_date }, quality?, audio_only?, start_sec?, end_sec? }
app.post('/download', async (req, res) => {
  const ctx = res.locals.ctx;
  const { url, tokens, quality = 'best', audio_only = false, start_sec, end_sec } = req.body || {};

  if (!url || !tokens?.access_token || !tokens?.refresh_token) {
    res.locals.release();
    return res.status(400).json({ error: 'url and tokens required' });
  }

  const videoId = extractVideoId(url);
  if (!videoId) {
    res.locals.release();
    return res.status(400).json({ error: 'Could not extract video ID from URL' });
  }

  const jobDir = join(DOWNLOAD_DIR, randomBytes(8).toString('hex'));

  let updatedTokens = null;

  try {
    mkdirSync(jobDir, { mode: 0o700 });
    const yt = await ctx.run(session(tokens, (credentials) => { updatedTokens = credentials; }, ctx));

    const info = await ctx.run(yt.getBasicInfo(videoId, 'TV'));
    // TV client with OAuth returns video_details without title/author.
    let title = info.basic_info?.title
      || info.page?.[0]?.video_details?.title
      || info.primary_info?.title?.runs?.map(r => r.text).join('');
    if (!title) {
      try {
        const ytWeb = await ctx.run(Innertube.create({ cache: new UniversalCache(false), generate_session_locally: true, fetch: ctx.fetch }));
        const webInfo = await ctx.run(ytWeb.getBasicInfo(videoId));
        title = webInfo.basic_info?.title || webInfo.page?.[0]?.video_details?.title;
      } catch { ctx.check(); }
    }
    title = title || videoId;
    const safeTitle = title.replace(/[^a-zA-Z0-9_\-\.]/g, '_').slice(0, 80);

    const ytQuality = resolveQuality(quality);

    let filePath;

    const clipStart = start_sec != null ? Number(start_sec) : null;
    const clipEnd   = end_sec   != null ? Number(end_sec)   : null;
    if ((clipStart !== null && (!Number.isFinite(clipStart) || clipStart < 0)) || (clipEnd !== null && (!Number.isFinite(clipEnd) || clipEnd <= (clipStart ?? 0)))) throw new Error('invalid clip');

    if (audio_only) {
      const rawPath = join(jobDir, `${safeTitle}_raw.m4a`);
      filePath = join(jobDir, `${safeTitle}.m4a`);
      const stream = await ctx.run(info.download({ type: 'audio', quality: ytQuality, format: 'any' }));
      await ctx.streamToFile(stream, rawPath);
      if (clipStart != null || clipEnd != null) {
        await ffmpegTrim(ctx, rawPath, filePath, clipStart, clipEnd);
      } else {
        filePath = rawPath;
      }
    } else {
      // TV client only has one progressive format (360p).
      // Download video and audio separately, merge with ffmpeg.
      const videoPath = join(jobDir, 'video.mp4');
      const audioPath = join(jobDir, 'audio.m4a');
      filePath = join(jobDir, `${safeTitle}.mp4`);

      // Prefer avc1 (H.264) to avoid Telegram transcoding; fall back to any mp4 if unavailable
      let videoStream;
      try {
        videoStream = await ctx.run(info.download({ type: 'video', quality: ytQuality, format: 'mp4', codec: 'avc' }));
      } catch {
        videoStream = await ctx.run(info.download({ type: 'video', quality: ytQuality, format: 'mp4' }));
      }
      await ctx.streamToFile(videoStream, videoPath);

      const audioStream = await ctx.run(info.download({
        type: 'audio',
        quality: 'best',
        format: 'any',
      }));
      await ctx.streamToFile(audioStream, audioPath);

      await ffmpegMerge(ctx, videoPath, audioPath, filePath, clipStart, clipEnd);
    }

    res.setHeader('Content-Disposition', `attachment; filename="${safeTitle}.${audio_only ? 'm4a' : 'mp4'}"`);
    res.setHeader('X-File-Title', encodeURIComponent(title));
    if (updatedTokens) {
      res.setHeader('X-Updated-Tokens', JSON.stringify(updatedTokens));
    }

    ctx.check();
    await ctx.run(new Promise((resolve, reject) => res.sendFile(filePath, { root: '/' }, (err) => err ? reject(err) : resolve())));

  } catch (err) {
    if (!res.headersSent && !res.destroyed) res.status(500).json({ error: 'download failed' });
    else res.destroy();
  } finally {
    try { rmSync(jobDir, { recursive: true, force: true }); } finally { res.locals.release(); }
  }
});

// GET /info
async function infoHandler(req, res) {
  const ctx = res.locals.ctx;
  const { url, tokens } = req.method === 'POST' ? (req.body || {}) : { url: req.query.url, tokens: req.query };
  const { access_token, refresh_token, expiry_date } = tokens || {};

  if (!url || !access_token || !refresh_token) {
    res.locals.release();
    return res.status(400).json({ error: 'url and tokens required' });
  }

  const videoId = extractVideoId(url);
  if (!videoId) { res.locals.release(); return res.status(400).json({ error: 'Invalid URL' }); }

  try {
    let refreshed = false;
    const yt = await ctx.run(session({ access_token, refresh_token, expiry_date }, () => { refreshed = true; }, ctx));
    if (req.method === 'POST' && refreshed) return res.status(409).json({ error: "credential refresh requires new admission" });
    const info = await ctx.run(yt.getBasicInfo(videoId, 'TV'));
    if (req.method === 'POST' && refreshed) return res.status(409).json({ error: 'credential refresh requires new admission' });
    res.json({
      id: videoId,
      title: info.basic_info?.title,
      duration: info.basic_info?.duration,
      author: info.basic_info?.author,
      is_age_restricted: info.basic_info?.is_age_restricted,
    });
  } catch (err) {
    if (!res.headersSent && !res.destroyed) res.status(500).json({ error: 'info failed' });
  } finally { res.locals.release(); }
}
app.get('/info', infoHandler);
app.post('/info', infoHandler);

app.get('/health', (_req, res) => res.json({ ok: true }));
app.use((err, req, res, next) => {
  res.locals.release?.();
  if (!res.headersSent) res.status(400).json({ error: 'invalid request' });
});
app.use((req, res) => { res.locals.release?.(); res.status(404).end(); });
return app;
}

function extractVideoId(url) {
  if (typeof url !== 'string') return null;
  const patterns = [
    /[?&]v=([a-zA-Z0-9_-]{11})/,
    /youtu\.be\/([a-zA-Z0-9_-]{11})/,
    /\/(?:shorts|embed|v|e)\/([a-zA-Z0-9_-]{11})/,
  ];
  for (const p of patterns) {
    const m = url.match(p);
    if (m) return m[1];
  }
  if (/^[a-zA-Z0-9_-]{11}$/.test(url)) return url;
  return null;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const settings = config();
  const server = createApp({ settings }).listen(settings.port, process.env.HOST);
  server.requestTimeout = settings.deadlineMs;
  server.headersTimeout = Math.min(30000, settings.deadlineMs);
}
