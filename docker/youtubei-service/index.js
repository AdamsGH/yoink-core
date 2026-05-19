import express from 'express';
import { Innertube, UniversalCache, Utils, Platform } from 'youtubei.js';
import { createWriteStream, mkdirSync, rmSync, existsSync } from 'fs';
import { join } from 'path';
import { randomBytes } from 'crypto';
import { spawn } from 'child_process';

// v17+: provide JS evaluator for deciphering streaming URLs
Platform.shim.eval = async (data) => new Function(data.output)();

const app = express();
app.use(express.json());

const PORT = process.env.PORT || 9173;
const DOWNLOAD_DIR = process.env.DOWNLOAD_DIR || '/tmp/youtubei';

if (!existsSync(DOWNLOAD_DIR)) mkdirSync(DOWNLOAD_DIR, { recursive: true });

async function makeInnertube(tokens) {
  console.log('[youtubei] creating session...');
  const yt = await Innertube.create({
    cache: new UniversalCache(false),
    generate_session_locally: true,
    client_type: 'TVHTML5',
    user_agent: 'Mozilla/5.0 (ChromiumStylePlatform) Cobalt/Version',
  });
  console.log('[youtubei] session created, signing in...');

  await new Promise((resolve, reject) => {
    yt.session.on('auth', () => { console.log('[youtubei] signed in'); resolve(); });
    yt.session.on('auth-error', (err) => reject(err));
    yt.session.signIn(tokens).catch(reject);
  });

  return yt;
}

async function streamToFile(stream, filePath) {
  const file = createWriteStream(filePath);
  for await (const chunk of Utils.streamToIterable(stream)) {
    file.write(chunk);
  }
  await new Promise((resolve, reject) => {
    file.end();
    file.on('finish', resolve);
    file.on('error', reject);
  });
}

function ffmpegRun(args) {
  return new Promise((resolve, reject) => {
    const proc = spawn('ffmpeg', args);
    proc.stderr.on('data', () => {});
    proc.on('close', (code) => {
      if (code === 0) resolve();
      else reject(new Error(`ffmpeg exited with code ${code}`));
    });
    proc.on('error', reject);
  });
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

function ffmpegMerge(videoPath, audioPath, outputPath, startSec, endSec) {
  const { pre, post } = buildClipArgs(startSec, endSec);
  const args = ['-y', ...pre, '-i', videoPath, '-i', audioPath, '-c:v', 'copy', '-c:a', 'copy', ...post, outputPath];
  return ffmpegRun(args);
}

function ffmpegTrim(inputPath, outputPath, startSec, endSec) {
  const { pre, post } = buildClipArgs(startSec, endSec);
  const args = ['-y', ...pre, '-i', inputPath, '-c', 'copy', ...post, outputPath];
  return ffmpegRun(args);
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

// POST /download
// Body: { url, tokens: { access_token, refresh_token, expiry_date }, quality?, audio_only?, start_sec?, end_sec? }
app.post('/download', async (req, res) => {
  const { url, tokens, quality = 'best', audio_only = false, start_sec, end_sec } = req.body;

  if (!url || !tokens?.access_token || !tokens?.refresh_token) {
    return res.status(400).json({ error: 'url and tokens required' });
  }

  const videoId = extractVideoId(url);
  if (!videoId) {
    return res.status(400).json({ error: 'Could not extract video ID from URL' });
  }

  const jobDir = join(DOWNLOAD_DIR, randomBytes(8).toString('hex'));
  mkdirSync(jobDir, { recursive: true });

  let updatedTokens = null;

  try {
    const yt = await makeInnertube(tokens);

    yt.session.on('update-credentials', ({ credentials }) => {
      updatedTokens = credentials;
    });

    console.log('[youtubei] fetching info for', videoId);
    const info = await yt.getBasicInfo(videoId, 'TV');
    // TV client with OAuth returns video_details without title/author.
    // Fetch metadata via a separate anonymous WEB client request.
    let title = info.basic_info?.title
      || info.page?.[0]?.video_details?.title
      || info.primary_info?.title?.runs?.map(r => r.text).join('');
    if (!title) {
      try {
        const ytWeb = await Innertube.create({ cache: new UniversalCache(false), generate_session_locally: true });
        const webInfo = await ytWeb.getBasicInfo(videoId);
        title = webInfo.basic_info?.title || webInfo.page?.[0]?.video_details?.title;
        console.log('[youtubei] fetched title via web client:', title);
      } catch (e) {
        console.warn('[youtubei] web client title fetch failed:', e.message);
      }
    }
    title = title || videoId;
    console.log('[youtubei] resolved title:', title);
    const safeTitle = title.replace(/[^a-zA-Z0-9_\-\.]/g, '_').slice(0, 80);

    const ytQuality = resolveQuality(quality);

    let filePath;

    const clipStart = start_sec != null ? Number(start_sec) : null;
    const clipEnd   = end_sec   != null ? Number(end_sec)   : null;

    if (audio_only) {
      const rawPath = join(jobDir, `${safeTitle}_raw.m4a`);
      filePath = join(jobDir, `${safeTitle}.m4a`);
      const stream = await info.download({ type: 'audio', quality: ytQuality, format: 'any' });
      await streamToFile(stream, rawPath);
      if (clipStart != null || clipEnd != null) {
        await ffmpegTrim(rawPath, filePath, clipStart, clipEnd);
      } else {
        filePath = rawPath;
      }
    } else {
      // TV client only has one progressive format (360p).
      // Download video and audio separately, merge with ffmpeg.
      const videoPath = join(jobDir, 'video.mp4');
      const audioPath = join(jobDir, 'audio.m4a');
      filePath = join(jobDir, `${safeTitle}.mp4`);

      console.log('[youtubei] downloading video stream, quality:', ytQuality);
      // Prefer avc1 (H.264) to avoid Telegram transcoding; fall back to any mp4 if unavailable
      let videoStream;
      try {
        videoStream = await info.download({ type: 'video', quality: ytQuality, format: 'mp4', codec: 'avc' });
      } catch {
        videoStream = await info.download({ type: 'video', quality: ytQuality, format: 'mp4' });
      }
      await streamToFile(videoStream, videoPath);
      console.log('[youtubei] video done, downloading audio...');

      const audioStream = await info.download({
        type: 'audio',
        quality: 'best',
        format: 'any',
      });
      await streamToFile(audioStream, audioPath);
      console.log('[youtubei] audio done, merging...');

      await ffmpegMerge(videoPath, audioPath, filePath, clipStart, clipEnd);
      console.log('[youtubei] merge done:', filePath);
    }

    res.setHeader('Content-Disposition', `attachment; filename="${safeTitle}.${audio_only ? 'm4a' : 'mp4'}"`);
    res.setHeader('X-File-Title', encodeURIComponent(title));
    if (updatedTokens) {
      res.setHeader('X-Updated-Tokens', JSON.stringify(updatedTokens));
    }

    res.sendFile(filePath, { root: '/' }, (err) => {
      try { rmSync(jobDir, { recursive: true, force: true }); } catch {}
      if (err && !res.headersSent) res.status(500).json({ error: String(err) });
    });

  } catch (err) {
    try { rmSync(jobDir, { recursive: true, force: true }); } catch {}
    console.error('Download error:', err);
    res.status(500).json({ error: err?.message ?? String(err) });
  }
});

// GET /info
app.get('/info', async (req, res) => {
  const { url, access_token, refresh_token, expiry_date } = req.query;

  if (!url || !access_token || !refresh_token) {
    return res.status(400).json({ error: 'url and tokens required' });
  }

  const videoId = extractVideoId(url);
  if (!videoId) return res.status(400).json({ error: 'Invalid URL' });

  try {
    const yt = await makeInnertube({ access_token, refresh_token, expiry_date });
    const info = await yt.getBasicInfo(videoId, 'TV');
    res.json({
      id: videoId,
      title: info.basic_info?.title,
      duration: info.basic_info?.duration,
      author: info.basic_info?.author,
      is_age_restricted: info.basic_info?.is_age_restricted,
    });
  } catch (err) {
    res.status(500).json({ error: err?.message ?? String(err) });
  }
});

app.get('/health', (_req, res) => res.json({ ok: true }));

function extractVideoId(url) {
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

app.listen(PORT, () => console.log(`youtubei-service listening on :${PORT}`));
