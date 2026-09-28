import assert from 'node:assert/strict';
import { mkdir, chmod, readdir, readFile } from 'node:fs/promises';
import path from 'node:path';

export async function createAdaptiveSource({ fixtureDir, helperContainer, image }) {
  for (const name of ['hls', 'dash']) {
    await mkdir(path.join(fixtureDir, name));
    await chmod(path.join(fixtureDir, name), 0o777);
  }
  const common = ['--network', 'none', '--entrypoint', 'ffmpeg', '--mount', `type=bind,source=${fixtureDir},target=/fixture`, image,
    '-hide_banner', '-loglevel', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=160x120:rate=25:duration=3',
    '-f', 'lavfi', '-i', 'sine=frequency=660:sample_rate=48000:duration=3', '-map', '0:v:0', '-map', '1:a:0',
    '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-g', '25', '-keyint_min', '25', '-sc_threshold', '0', '-c:a', 'aac'];
  await helperContainer([...common, '-f', 'hls', '-hls_time', '1', '-hls_playlist_type', 'vod',
    '-hls_segment_filename', '/fixture/hls/segment-%02d.ts', '/fixture/hls/index.m3u8'], 20000);
  await helperContainer([...common, '-f', 'dash', '-seg_duration', '1', '-use_timeline', '1', '-use_template', '1',
    '-init_seg_name', 'init-$RepresentationID$.m4s', '-media_seg_name', 'chunk-$RepresentationID$-$Number%05d$.m4s', '/fixture/dash/index.mpd'], 20000);
  const files = new Map();
  for (const name of ['hls', 'dash']) {
    for (const filename of await readdir(path.join(fixtureDir, name))) {
      const bytes = await readFile(path.join(fixtureDir, name, filename));
      assert(bytes.length > 0 && bytes.length < 1024 * 1024);
      files.set(`${name}/${filename}`, bytes);
    }
  }
  files.set('hls/master.m3u8', Buffer.from('#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=250000,RESOLUTION=160x120,CODECS="avc1.64000b,mp4a.40.2"\nindex.m3u8\n'));
  return new AdaptiveSource(files);
}

export class AdaptiveSource {
  requests = [];
  constructor(files) { this.files = files; }
  respond(req, res, url) {
    if (!url.pathname.startsWith('/adaptive/')) return false;
    const match = url.pathname.match(/^\/adaptive\/(good|missing|private-redirect|private-key)\/(hls|dash)\/([A-Za-z0-9._-]+)$/);
    if (!match) { res.writeHead(404); res.end(); return true; }
    const [, scenario, kind, filename] = match;
    this.requests.push({ scenario, kind, filename, method: req.method });
    const second = filename === 'segment-01.ts' || filename === 'chunk-0-00002.m4s';
    if (second && scenario === 'missing') { res.writeHead(404); res.end(); return true; }
    if (second && scenario === 'private-redirect') {
      res.writeHead(302, { location: 'http://169.254.169.254/blocked-segment' }); res.end(); return true;
    }
    let bytes = this.files.get(`${kind}/${filename}`);
    if (bytes && scenario === 'private-key' && kind === 'hls' && filename === 'index.m3u8') {
      bytes = Buffer.from(bytes.toString().replace('#EXTM3U\n', '#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI="http://169.254.169.254/blocked-key"\n'));
    }
    if (!bytes) { res.writeHead(404); res.end(); return true; }
    const type = filename.endsWith('.m3u8') ? 'application/vnd.apple.mpegurl' : filename.endsWith('.mpd') ? 'application/dash+xml' : filename.endsWith('.ts') ? 'video/mp2t' : 'video/mp4';
    res.writeHead(200, { 'content-type': type, 'content-length': bytes.length });
    res.end(req.method === 'HEAD' ? undefined : bytes);
    return true;
  }
}
