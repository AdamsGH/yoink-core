// Native fixture metadata for the actual pinned YouTube search/video extractors.
// Production requests retain real YouTube URLs; an operator proxy terminates TLS
// at this fixture, with exact source origins and a fixture-only trusted CA.
import assert from 'node:assert/strict';

export function searchVideo(index, title = `Unicode кот ${index}`) {
  const id = `fixture${String(index).padStart(4, '0')}`;
  return { videoRenderer: { videoId: id, title: { runs: [{ text: title }] },
    ownerText: { runs: [{ text: 'Fixture channel', navigationEndpoint: { browseEndpoint: { browseId: 'UCfixture' } } }] },
    lengthText: { simpleText: '0:01' }, viewCountText: { simpleText: '1,234 views' },
    navigationEndpoint: { watchEndpoint: { videoId: id } } } };
}
export function searchMetadata(query) {
  const videos = query === 'zero' ? [] : query === 'duplicate' ? Array(8).fill(searchVideo(0))
    : Array.from({ length: query === 'oversized' ? 20 : 8 }, (_, i) => searchVideo(i));
  return { contents: { twoColumnSearchResultsRenderer: { primaryContents: { sectionListRenderer: {
    contents: [{ itemSectionRenderer: { contents: videos } }] } } } } };
}
export function playerMetadata(id, mediaUrl, size) {
  return { playabilityStatus: { status: 'OK' },
    videoDetails: { videoId: id, title: 'Ordinary inline fixture', lengthSeconds: '1', channelId: 'UCfixture', author: 'Fixture channel', shortDescription: 'Ordinary fixture', viewCount: '1234', isLiveContent: false },
    microformat: { playerMicroformatRenderer: { uploadDate: '2020-01-01', publishDate: '2020-01-01', category: 'Education' } },
    streamingData: { expiresInSeconds: '3600', formats: [{ itag: 18, url: mediaUrl,
      mimeType: 'video/mp4; codecs="avc1.42001E, mp4a.40.2"', bitrate: 200000, width: 160, height: 120,
      contentLength: String(size), quality: 'medium', qualityLabel: '144p', fps: 25, approxDurationMs: '1000',
      audioQuality: 'AUDIO_QUALITY_LOW', audioSampleRate: '44100', audioChannels: 1 }] } };
}
export class InlineSource {
  trace = [];
  gates = new Map();
  constructor({ mediaUrl, mediaBytes }) { this.mediaUrl = mediaUrl; this.mediaBytes = mediaBytes; }
  hold(query) {
    assert(!this.gates.has(query)); let release;
    const promise = new Promise(resolve => { release = resolve; });
    this.gates.set(query, { promise, release });
    return () => { release(); this.gates.delete(query); };
  }
  releaseAll() { for (const gate of this.gates.values()) gate.release(); this.gates.clear(); }
  response(host, path, { body = '', headers = {}, method = 'GET' } = {}) {
    if (!['www.youtube.com', 'youtube.com'].includes(host)) return;
    const url = new URL(path, `https://${host}`);
    // The pinned extractor sets public language/consent preferences itself.
    // Authentication cookies must never be borrowed from the user or pool.
    if (headers.cookie) assert(headers.cookie.split(/;\s*/).every(cookie => /^(PREF|SOCS)=/.test(cookie)), 'metadata request borrowed authentication cookies');
    assert.equal(headers['proxy-authorization'], undefined, 'proxy credentials must not reach metadata origin');
    const data = body ? JSON.parse(body) : {};
    this.trace.push({ path: url.pathname, query: data.query, id: data.videoId, method });
    const json = value => ({ status: 200, type: 'application/json', body: JSON.stringify(value) });
    if (url.pathname === '/youtubei/v1/search') {
      if (data.query === 'network denial') return { status: 302, headers: { location: 'http://127.0.0.2:1/private-secret' }, body: '' };
      if (data.query === 'response overflow') return { status: 200, type: 'application/json', body: 'x'.repeat(3 * 1024 * 1024) };
      if (data.query === 'failure') return { status: 503, body: 'fixture source failure' };
      return { ...json(searchMetadata(data.query)), gate: this.gates.get(data.query)?.promise };
    }
    const id = data.videoId ?? url.searchParams.get('v') ?? 'fixture0000';
    const player = playerMetadata(id, this.mediaUrl + '&id=' + id, this.mediaBytes.length);
    if (url.pathname === '/youtubei/v1/player') return json(player);
    if (url.pathname === '/watch') return { status: 200, type: 'text/html', body:
      `<html><head><title>Fixture</title></head><body><script>var ytInitialPlayerResponse = ${JSON.stringify(player)}; var ytInitialData = {"contents":{"twoColumnWatchNextResults":{}}};</script><script>ytcfg.set({"INNERTUBE_CLIENT_NAME":"WEB","INNERTUBE_CLIENT_VERSION":"2.20250101.00.00","INNERTUBE_API_KEY":"fixture","STS":20000});</script></body></html>` };
    return { status: 404, body: 'unsupported fixture metadata path' };
  }
}
