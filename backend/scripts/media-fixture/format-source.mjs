// Metadata fixtures deliberately never serve media payloads.
export class FormatSource {
  requests = [];
  gated = new Set();
  constructor(cookieSecret) { this.cookieSecret = cookieSecret; }
  release() {
    for (const send of this.gated) send();
    this.gated.clear();
  }
  respond(req, res, url) {
    if (!url.pathname.startsWith('/format-fixture/')) return false;
    this.requests.push({ path: url.pathname, method: req.method });
    if (url.pathname.includes('/protected/') && !req.headers.cookie?.includes(`session=${this.cookieSecret}`)) {
      res.writeHead(403); res.end(); return true;
    }
    if (url.pathname.endsWith('/redirect.m3u8')) {
      res.writeHead(302, { location: 'http://169.254.169.254/forbidden-format-source' }); res.end(); return true;
    }
    if (url.pathname.endsWith('/direct.mp4')) {
      // Declared media body is intentionally absent: header-only discovery can succeed,
      // while any attempt to read/download it fails with an incomplete response.
      res.writeHead(200, { 'content-type': 'video/mp4', 'content-length': 10000 });
      res.end(); return true;
    }
    let body, type = 'application/vnd.apple.mpegurl';
    if (url.pathname.endsWith('/master.m3u8')) {
      body = '#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1200000,RESOLUTION=1280x720,CODECS="avc1.64001f,mp4a.40.2"\nindex.m3u8?private=manifest-secret\n#EXT-X-STREAM-INF:BANDWIDTH=300000,RESOLUTION=640x360,CODECS="avc1.4d401e,mp4a.40.2"\nlow.m3u8\n';
    } else if (/\/(index|low)\.m3u8$/.test(url.pathname)) {
      body = '#EXTM3U\n#EXT-X-TARGETDURATION:3\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:3.0,\nforbidden-payload.ts?token=payload-secret\n#EXT-X-ENDLIST\n';
    } else if (url.pathname.endsWith('/playlist.html')) {
      type = 'text/html';
      body = '<html><head><title>metadata-title-secret</title></head><body><video src="forbidden-one.mp4"></video><video src="forbidden-two.mp4"></video></body></html>';
    } else {
      // If metadata discovery tries to download payloads, fail rather than help it succeed.
      res.writeHead(404); res.end(); return true;
    }
    const send = () => {
      if (res.destroyed) return;
      res.writeHead(200, { 'content-type': type, 'content-length': Buffer.byteLength(body) });
      res.end(req.method === 'HEAD' ? undefined : body);
    };
    if (url.searchParams.get('gate') === 'true') {
      this.gated.add(send);
      res.once('close', () => this.gated.delete(send));
    } else send();
    return true;
  }
}
