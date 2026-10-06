import assert from 'node:assert/strict';
import { galleryImage } from './gallery-source.mjs';

export const INSTAGRAM_CASES = Object.freeze({
  PRMeta: 'metadata-failure', PRJson: 'invalid-json', PRDownload: 'download-failure', PRSuccess: 'primary-success',
  PRBoth: 'both-fail', PRLimit: 'primary-limit', PRRedirect: 'confinement',
  PRItems: 'gallery-item-limit', PRCookie: 'cookie-revocation', PRRole: 'role-revocation',
  PRCancel: 'cancellation', PRAbort: 'primary-cancellation', PRRestart: 'restart', PRNsfw: 'nsfw', PRPolicy: 'policy-revocation',
  PRBlocker: 'blocker',
});

// The installed extractors use the same API endpoint but distinct documented X-ASBD-ID headers.
// These are response fixtures only: no custom extractor, engine override, or admission authority.
export function instagramMediaId(shortcode) {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';
  return [...shortcode].reduce((id, char) => {
    const digit = alphabet.indexOf(char);
    assert(digit >= 0);
    return id * 64n + BigInt(digit);
  }, 0n).toString();
}
const codesById = new Map(Object.keys(INSTAGRAM_CASES).map(code => [instagramMediaId(code), code]));
const json = value => ({ status: 200, type: 'application/json', body: JSON.stringify(value) });
const missing = () => ({ status: 404, body: 'fixture missing resource' });

function instagramProduct(code, provider) {
  const gallery = provider === 'gallery';
  const id = instagramMediaId(code);
  const mode = INSTAGRAM_CASES[code];
  const image = num => ({ pk: (BigInt(id) + BigInt(num)).toString(), media_type: 1,
    image_versions2: { candidates: [{ width: 160, height: 120, url: `https://imgbox.com/routing/${code}/gallery-${num}.png` }] } });
  const video = { ...image(2), media_type: 2, video_codec: 'avc1.64000a', has_audio: true, video_duration: 1,
    video_versions: [{ id: 'fixture-video', type: 101, width: 160, height: 120,
      url: `https://imgbox.com/routing/${code}/${gallery ? 'gallery' : 'primary'}.mp4` }] };
  const caption = gallery ? (['nsfw', 'policy-revocation'].includes(mode) ? 'sensitivecontent gallery evidence' : 'ordinary gallery evidence') : 'sensitivecontent primary-only evidence';
  const post = { ...video, pk: id, code, title: gallery ? 'Gallery fixture' : 'Primary fixture',
    user: { pk: '42', username: 'fixture', full_name: 'Fixture' }, taken_at: 1700000000,
    caption: { text: caption }, like_count: 0, comment_count: 0 };
  if (gallery) post.carousel_media = mode === 'gallery-item-limit' ? Array.from({ length: 21 }, (_, i) => image(i + 1)) : [image(1), video];
  return post;
}

// Pure installed 4chan JSON contract retained for mixed image/video and explicit /image acceptance.
export function providerRoutingResponse(mediaBytes, host, path) {
  if (host === 'a.4cdn.org') {
    const match = path.match(/^\/test\/thread\/(700[12])\.json$/);
    if (!match) return missing();
    const thread = Number(match[1]);
    return json({ posts: [
      { no: thread, sub: 'ordinary fixture', com: 'fixture', filename: 'first', tim: thread * 10 + 1, ext: '.png' },
      { no: thread + 1, filename: 'second', tim: thread * 10 + 2, ext: '.mp4' },
    ] });
  }
  if (host === 'i.4cdn.org') {
    if (path === '/test/70011.png' || path === '/test/70021.png') return { status: 200, type: 'image/png', body: galleryImage(1) };
    if (path === '/test/70012.mp4') return { status: 200, type: 'video/mp4', body: mediaBytes };
    return missing();
  }
}

export class ProviderRoutingSource {
  #gates = new Map();
  #trace = [];
  constructor({ mediaBytes, primaryBytes = mediaBytes, cookieSecret, maxBytes = 65536 }) {
    this.mediaBytes = mediaBytes;
    this.primaryBytes = primaryBytes;
    this.cookieSecret = cookieSecret;
    this.maxBytes = maxBytes;
  }
  get trace() { return [...this.#trace]; }
  events(code, stage) { return this.#trace.filter(event => event.code === code && (!stage || event.stage === stage)); }
  hold(code, stage) {
    assert(INSTAGRAM_CASES[code] && ['primary-media', 'gallery-media', 'gallery-api'].includes(stage));
    const key = `${code}:${stage}`;
    assert(!this.#gates.has(key));
    let release;
    const promise = new Promise(resolve => { release = resolve; });
    this.#gates.set(key, { promise, release });
    return () => { release(); this.#gates.delete(key); };
  }
  releaseAll() { for (const gate of this.#gates.values()) gate.release(); this.#gates.clear(); }
  response(host, path, { headers = {}, method = 'GET' } = {}) {
    const fourchan = providerRoutingResponse(this.mediaBytes, host, path);
    if (fourchan) return fourchan;
    const url = new URL(path, `https://${host}`);
    const api = url.pathname.match(/^\/api\/v1\/media\/(\d+)\/info\/$/);
    if (host === 'www.instagram.com' && api) {
      const code = codesById.get(api[1]);
      if (!code) return missing();
      const provider = headers['x-asbd-id'] === '359341' ? 'primary' : headers['x-asbd-id'] === '129477' ? 'gallery' : undefined;
      const authorized = String(headers.cookie ?? '').split(';').some(value => value.trim() === `sessionid=${this.cookieSecret}`);
      this.#trace.push({ code, stage: `${provider ?? 'unknown'}-api`, method, authorized });
      if (!provider || !authorized) return { status: 403, body: 'fixture session required' };
      const mode = INSTAGRAM_CASES[code];
      if (provider === 'primary' && mode === 'invalid-json') return { status: 200, type: 'application/json', body: '{"items":' };
      if (mode === 'both-fail' || (provider === 'primary' && !['download-failure', 'primary-success', 'primary-limit', 'confinement', 'blocker', 'primary-cancellation'].includes(mode))) return missing();
      const response = json({ status: 'ok', items: [instagramProduct(code, provider)], more_available: false });
      return { ...response, gate: this.#gates.get(`${code}:${provider}-api`)?.promise };
    }
    if (host === 'imgbox.com') {
      const media = url.pathname.match(/^\/routing\/(PR[A-Za-z]+)\/(primary\.mp4|gallery\.mp4|gallery-(\d+)\.png)$/);
      if (!media || !INSTAGRAM_CASES[media[1]]) return undefined;
      const code = media[1];
      const provider = media[2] === 'primary.mp4' ? 'primary' : 'gallery';
      const stage = `${provider}-media`;
      this.#trace.push({ code, stage, path: url.pathname, method });
      const mode = INSTAGRAM_CASES[code];
      if (provider === 'primary' && mode === 'confinement') return { status: 302, headers: { location: 'http://127.0.0.1:1/forbidden.mp4' }, body: '' };
      if (provider === 'primary' && mode === 'primary-limit') return { status: 200, type: 'video/mp4', body: this.primaryBytes, declaredLength: this.maxBytes + 1, truncateAt: 1024 };
      if (provider === 'primary' && mode === 'download-failure') return { status: 200, type: 'video/mp4', body: this.primaryBytes, truncateAt: Math.min(2048, this.primaryBytes.length - 1) };
      const body = media[3] ? galleryImage(Number(media[3])) : provider === 'primary' ? this.primaryBytes : this.mediaBytes;
      // Hold the final video, not the image prefix, for delivery-time credential/policy checks.
      const gate = !media[3] ? this.#gates.get(`${code}:${stage}`)?.promise : undefined;
      return { status: 200, type: media[3] ? 'image/png' : 'video/mp4', body, gate };
    }
    if (host === 'www.instagram.com' || host === 'instagram.com') return missing();
  }
}
