import assert from 'node:assert/strict';
import { commandHarness } from './command-harness.mjs';

export async function liveYoutubeConfiguration(docker, registerSecret) {
  const env = new Map(JSON.parse(await docker(['inspect', 'yoink', '--format', '{{json .Config.Env}}'])).map(value => {
    const index = value.indexOf('=');
    return [value.slice(0, index), value.slice(index + 1)];
  }));
  const setting = name => env.get(`YOINK_RUST_${name}`) ?? env.get(`YOINK_RUST_YOINK_RUST_${name}`);
  const proxy = setting('DOWNLOAD_PROXY_URL');
  const domains = setting('DOWNLOAD_PROXY_DOMAINS_JSON');
  const allowed = env.get('YOINK_RUST_WORKER_ALLOWED_PROXY_ORIGINS');
  assert(proxy && domains && allowed, 'live check requires effective production proxy configuration');
  registerSecret(proxy);
  const origin = new URL(proxy);
  assert.equal(origin.hostname, 'yoink-proxy', 'live check requires the project proxy facade');
  assert(!origin.username && !origin.password, 'live check requires the credential-free internal proxy facade');
  assert(JSON.parse(domains).includes('youtube.com'), 'production must automatically proxy YouTube');
  assert(JSON.parse(allowed).includes(origin.origin), 'production must admit only the configured proxy origin');
  const networks = JSON.parse(await docker(['inspect', 'yoink-proxy', '--format', '{{json .NetworkSettings.Networks}}']));
  const address = Object.values(networks).map(network => network.IPAddress).find(Boolean);
  assert(address, 'project proxy must have an active network address');
  return { args: [
    '--add-host', `yoink-proxy:${address}`,
    '--env', `YOINK_RUST_DOWNLOAD_PROXY_URL=${proxy}`,
    '--env', `YOINK_RUST_DOWNLOAD_PROXY_DOMAINS_JSON=${domains}`,
    '--env', `YOINK_RUST_WORKER_ALLOWED_PROXY_ORIGINS=${allowed}`,
    '--env', `YOINK_RUST_WORKER_ALLOWED_PRIVATE_ORIGINS=${env.get('YOINK_RUST_WORKER_ALLOWED_PRIVATE_ORIGINS') ?? '[]'}`,
    '--env', `YOINK_RUST_DOWNLOAD_TIMEOUT_SECS=${env.get('YOINK_RUST_DOWNLOAD_TIMEOUT_SECS') ?? '120'}`,
    '--env', `YOINK_RUST_WORKER_SOCKET_TIMEOUT_SECS=${env.get('YOINK_RUST_WORKER_SOCKET_TIMEOUT_SECS') ?? '15'}`,
    '--env', 'YOINK_RUST_DOWNLOAD_MAX_BYTES=67108864',
    '--env', 'YOINK_RUST_DOWNLOAD_WORKSPACE_MAX_BYTES=201326592',
    '--env', 'YOINK_RUST_DOWNLOAD_LEASE_SECS=30',
    '--env', 'YOINK_RUST_DELIVERY_TIMEOUT_SECS=15',
  ] };
}

export async function runLiveYoutubeScenario(context) {
  const { botApi, fixtureSql, waitFor, queryJob, helperContainer, image, assertWorkspaceClean } = context;
  const url = process.env.YOINK_MEDIA_LIVE_YOUTUBE_URL;
  assert(url && ['www.youtube.com', 'youtube.com', 'youtu.be'].includes(new URL(url).hostname), 'supply a public YouTube URL');
  assert.equal(await fixtureSql('SELECT count(*) FROM dl_user_settings WHERE user_id=42 AND proxy_enabled=1'), '0');
  const h = commandHarness(context, 97000);
  botApi.setExpectedUpload({ method: 'sendvideo', filename: 'media.mp4', mimeType: 'video/mp4', preserveMdat: false });
  try {
    const admitted = await h.submit(`/video --height 360 --video-codec h264 --container mp4 --send-as-file false ${url}`);
    assert((await h.request(admitted.operationId)).network.proxy, 'ordinary admission must freeze the automatic system proxy');
    const state = await waitFor(async () => {
      const row = await queryJob(admitted.operationId);
      return /^(succeeded|failed|cancelled|delivery_unknown)\|/.test(row) ? row : false;
    }, 'real YouTube download and fixture upload', 210000);
    assert.match(state, /^succeeded\|/, `real YouTube operation ended as ${state}`);
    assert.equal(botApi.uploadCalls, 1, 'live check must upload exactly once to the isolated Bot API');
    const media = botApi.lastUploadedBytes;
    assert(media?.length > 100 && media.length <= 64 * 1024 * 1024);
    const probe = JSON.parse(await helperContainer(['--network', 'none', '--entrypoint', 'ffprobe', image,
      '-v', 'error', '-show_entries', 'format=duration:stream=codec_type,height', '-of', 'json', '-i', 'pipe:0'], 15000, media, { allowEarlyStdinClose: true }));
    assert(probe.streams.some(stream => stream.codec_type === 'video'));
    assert(probe.streams.some(stream => stream.codec_type === 'audio'));
    await assertWorkspaceClean();
    assert.equal(botApi.error, undefined);
    return `live YouTube automatic route, native admission, real confined download and isolated multipart upload verified (${media.length} bytes)`;
  } finally {
    botApi.setExpectedUpload(null);
  }
}
