export function subtitleBytes(language, extension = 'vtt') {
  return Buffer.from(extension === 'srt'
    ? `1\r\n00:00:00,000 --> 00:00:02,000\r\nFixture ${language} subtitle\r\n`
    : `WEBVTT\n\n00:00.000 --> 00:02.000\nFixture ${language} subtitle\n`);
}

export function subtitleResponse(url) {
  if (url.pathname === '/subtitles.html') {
    const extension = url.searchParams.get('format') === 'srt' ? 'srt' : 'vtt';
    const tracks = (url.searchParams.get('tracks') ?? 'en,ru').split(',').filter(code => /^[a-z]{2,3}(?:-[A-Za-z0-9]{2,8})?$/.test(code)).slice(0, 4);
    const query = `case=${encodeURIComponent(url.searchParams.get('case') ?? 'normal')}`;
    return { type: 'text/html', body: Buffer.from(`<html><head><title>Subtitle fixture</title></head><body><video controls src="/subtitle-video.mp4">${tracks.map(code => `<track kind="subtitles" srclang="${code}" src="/subtitles-${code}.${extension}?${query}">`).join('')}</video></body></html>`) };
  }
  const track = url.pathname.match(/^\/subtitles-([A-Za-z0-9-]+)\.(vtt|srt)$/);
  if (!track) return null;
  const scenario = url.searchParams.get('case');
  if (scenario === 'missing' && track[1].endsWith('-orig')) return { status: 404, body: Buffer.from('missing') };
  if (scenario === 'invalid') return { type: 'text/vtt', body: Buffer.from('<html>not subtitles</html>') };
  if (scenario === 'oversized') return { type: 'text/vtt', body: Buffer.alloc(4 * 1024 * 1024 + 1, 'x'), chunked: true };
  if (scenario === 'private-redirect') return { status: 302, location: 'http://169.254.169.254/blocked-subtitle', body: Buffer.alloc(0) };
  return { type: track[2] === 'vtt' ? 'text/vtt' : 'application/x-subrip', body: subtitleBytes(track[1], track[2]) };
}
