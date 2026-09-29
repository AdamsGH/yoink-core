export function subtitleBytes(language, extension = 'vtt') {
  if (extension === 'srt') return Buffer.from(`1\r\n00:00:00,000 --> 00:00:02,000\r\nFixture ${language} subtitle\r\n`);
  if (extension === 'ass' || extension === 'ssa') {
    const ass = extension === 'ass';
    return Buffer.from(`${ass ? '' : '\uFEFF'}[Script Info]\r\nTitle: Fixture ${language}\r\nScriptType: v${ass ? '4.00+' : '4.00'}\r\n; Plain document, no rendering\r\n\r\n[Events]\r\nFormat: ${ass ? 'Layer' : 'Marked'}, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\r\nDialogue: ${ass ? '0' : 'Marked=0'},0:00:00.00,0:00:02.00,Default,,0,0,0,,Fixture ${language} subtitle, Привет {\\i1}world{\\i0}\r\n`);
  }
  return Buffer.from(`WEBVTT\n\n00:00.000 --> 00:02.000\nFixture ${language} subtitle\n`);
}

export function subtitleResponse(url) {
  if (url.pathname === '/subtitles.html') {
    const formats = (url.searchParams.get('formats') ?? url.searchParams.get('format') ?? 'vtt').split(',').filter(format => ['vtt', 'srt', 'ass', 'ssa'].includes(format)).slice(0, 4);
    const tracks = (url.searchParams.get('tracks') ?? 'en,ru').split(',').filter(code => /^[a-z]{2,3}(?:-[A-Za-z0-9]{2,8})?$/.test(code)).slice(0, 4);
    const query = `case=${encodeURIComponent(url.searchParams.get('case') ?? 'normal')}`;
    return { type: 'text/html', body: Buffer.from(`<html><head><title>Subtitle fixture</title></head><body><video controls src="/subtitle-video.mp4">${tracks.flatMap(code => formats.map(extension => `<track kind="subtitles" srclang="${code}" src="/subtitles-${code}.${extension}?${query}">`)).join('')}</video></body></html>`) };
  }
  const track = url.pathname.match(/^\/subtitles-([A-Za-z0-9-]+)\.(vtt|srt|ass|ssa)$/);
  if (!track) return null;
  const scenario = url.searchParams.get('case');
  if (scenario === 'missing' && track[1].endsWith('-orig')) return { status: 404, body: Buffer.from('missing') };
  if (scenario === 'invalid') return { type: 'text/vtt', body: Buffer.from('<html>not subtitles</html>') };
  if (scenario === 'invalid-utf8') return { type: 'text/x-ssa', body: Buffer.from([0xff, 0xfe, 0xfa]) };
  if (scenario === 'malformed-events') return { type: 'text/x-ssa', body: Buffer.from('[Script Info]\nTitle: malformed\n') };
  if (scenario === 'malformed-timing') return { type: 'text/x-ssa', body: Buffer.from('[Script Info]\nTitle: malformed\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,not-a-time,0:00:02.00,Default,,0,0,0,,Broken\n') };
  if (scenario === 'bad-second' && track[1] === 'ru-orig') return { type: 'text/x-ssa', body: Buffer.from('[Script Info]\nTitle: malformed second track\n') };
  if (scenario === 'oversized') return { type: 'text/vtt', body: Buffer.alloc(4 * 1024 * 1024 + 1, 'x'), chunked: true };
  if (scenario === 'private-redirect') return { status: 302, location: 'http://169.254.169.254/blocked-subtitle', body: Buffer.alloc(0) };
  return { type: track[2] === 'vtt' ? 'text/vtt' : track[2] === 'srt' ? 'application/x-subrip' : 'text/x-ssa', body: subtitleBytes(track[1], track[2]) };
}
