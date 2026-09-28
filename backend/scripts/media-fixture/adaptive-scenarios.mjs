import assert from 'node:assert/strict';
import { commandHarness } from './command-harness.mjs';

export async function runAdaptiveScenarios(context) {
  const { botApi, sourceFixture, sourcePort, fixtureSql, queryJob, waitFor, assertWorkspaceClean, restartApp } = context;
  const h = commandHarness(context, 4000);
  const source = sourceFixture.adaptiveSource;
  const url = (kind, scenario = 'good') => `http://127.0.0.1:${sourcePort}/adaptive/${scenario}/${kind}/${kind === 'hls' ? 'master.m3u8' : 'index.mpd'}`;
  const video = (kind, scenario) => `/video ${url(kind, scenario)} --height best --video-codec h264 --container mp4 --send-as-file false`;
  try {
    for (const kind of ['hls', 'dash']) {
      const before = source.requests.length;
      const sent = await h.execute(`${kind} segmented video`, video(kind), 'sendvideo',
        { video: true, audio: true, durationMin: 2.8, durationMax: 3.3, height: 120 },
        { filename: 'media.mp4', mimeType: 'video/mp4' });
      const fetched = source.requests.slice(before).filter(r => r.method === 'GET');
      if (kind === 'hls') {
        for (const filename of ['index.m3u8', 'segment-00.ts', 'segment-01.ts', 'segment-02.ts']) assert(fetched.some(r => r.filename === filename), `HLS must fetch ${filename}`);
      } else {
        for (const filename of ['init-0.m4s', 'init-1.m4s', 'chunk-0-00002.m4s', 'chunk-1-00002.m4s']) assert(fetched.some(r => r.filename === filename), `DASH must fetch ${filename}`);
      }
      await h.replay(video(kind), sent);
      await restartApp();
      const requests = source.requests.length, cached = botApi.cachedCalls;
      const reused = await h.submit(video(kind)); await h.succeeded(reused.operationId);
      assert.equal(source.requests.length, requests);
      assert.equal(botApi.cachedCalls, cached + 1);
      await h.execute(`${kind} audio extraction`, `/audio ${url(kind)} --audio-codec aac --send-as-file false`, 'sendaudio',
        { video: false, audio: true, durationMin: 2.8, durationMax: 3.3, audioCodec: 'aac' },
        { filename: 'audio.m4a', mimeType: 'audio/mp4' });
      for (const scenario of ['missing', 'private-redirect', ...(kind === 'hls' ? ['private-key'] : [])]) {
        const sends = botApi.sendCalls;
        botApi.setExpectedUpload({ method: 'sendvideo', filename: 'media.mp4', mimeType: 'video/mp4', preserveMdat: false });
        const failed = await h.submit(video(kind, scenario));
        const row = await waitFor(async () => {
          const row = await queryJob(failed.operationId);
          return /^(failed|succeeded|delivery_unknown|partially_delivered)\|/.test(row) ? row : false;
        }, `${kind} ${scenario} terminal state`, 25000);
        assert.match(row, /^failed\|/, `${kind} ${scenario} must fail before delivery`);
        assert.equal(botApi.sendCalls, sends, 'unavailable segments must never produce partial media delivery');
        assert.equal(await fixtureSql(`SELECT count(*) FROM runtime_delivery_units WHERE operation_id='${failed.operationId}'`), '0');
        await assertWorkspaceClean();
      }
    }
  } finally {
    botApi.setExpectedUpload(null);
  }
  return 'local HLS MPEG-TS and separate-stream DASH segments, real video/audio output, replay/cache after restart and no partial sends on missing/private segments or private HLS keys passed';
}
