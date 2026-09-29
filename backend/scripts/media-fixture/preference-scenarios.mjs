import assert from 'node:assert/strict';
import { commandHarness } from './command-harness.mjs';

const fields = ['quality', 'codec', 'container', 'audio_codec', 'send_as_file', 'gallery_zip'];
const literal = value => typeof value === 'number' ? String(value) : `'${value.replaceAll("'", "''")}'`;

export async function runPreferenceScenarios(context) {
  const { botApi, sourcePort, sourceFixture, fixtureSql, queryJob, waitFor, assertWorkspaceClean } = context;
  const { submit, succeeded, cancel, execute, inspectUpload, request, reject, replay } = commandHarness(context, 1100);
  const sourceUrl = `http://127.0.0.1:${sourcePort}/media?case=preference-acceptance`;
  const previousText = await fixtureSql(`SELECT json_object(${fields.map(f => `'${f}', ${f}`).join(', ')}) FROM dl_user_settings WHERE user_id = 42`);
  const previous = previousText.trim() ? JSON.parse(previousText) : null;
  async function cacheKey(id) {
    return fixtureSql(`SELECT cache_key FROM runtime_operations WHERE id = '${id}'`);
  }
  try {
    await fixtureSql(`INSERT INTO dl_user_settings (user_id, quality, codec, container, proxy_enabled, keyboard, subs_enabled, subs_auto, subs_always_ask, subs_lang, split_size, nsfw_blur, mediainfo, send_as_file, audio_codec, gallery_zip, use_pool_cookies, youtube_auth_mode, args_json, created_at, updated_at) VALUES (42, '60', 'avc1', 'mp4', 0, '2x3', 0, 0, 0, 'en', 2043000000, 1, 0, 1, 'best', 0, 1, 'cookies', '{}', unixepoch('now')*1000000, unixepoch('now')*1000000) ON CONFLICT(user_id) DO UPDATE SET quality = '60', codec = 'avc1', container = 'mp4', audio_codec = 'best', send_as_file = 1, gallery_zip = 0`);
    const videoCommand = `/video ${sourceUrl}`;
    const original = await execute('saved video preferences', videoCommand, 'senddocument',
      { video: true, audio: true, height: 60, durationMin: 0.8, durationMax: 1.5 },
      { filename: 'media.mp4', mimeType: 'video/mp4' });
    const originalRequest = await request(original.operationId);
    assert.equal(originalRequest.params.max_height, 60);
    assert.equal(originalRequest.params.send_as_file, true);

    // Replays must not consult a now-unusable saved quality.
    await fixtureSql("UPDATE dl_user_settings SET quality = 'bogus', send_as_file = 0 WHERE user_id = 42");
    await replay(videoCommand, original);
    assert.deepEqual(await request(original.operationId), originalRequest);
    await reject(videoCommand, 'Check your download settings before retrying.');
    const overridden = await execute('explicit best and false',
      `/video --height best --send-as-file false ${sourceUrl}`, 'sendvideo',
      { video: true, audio: true, height: 120, durationMin: 0.8, durationMax: 1.5 });
    const overriddenRequest = await request(overridden.operationId);
    assert.equal(overriddenRequest.params.max_height, null);
    assert.equal(overriddenRequest.params.send_as_file, false);
    assert.notEqual(await cacheKey(original.operationId), await cacheKey(overridden.operationId),
      'different effective format parameters must not share a media cache key');
    await reject(`/video --height best ${sourceUrl}`, 'This request conflicts with an existing operation.', original.messageId);
    await reject(`/video --height 60 --height 120 ${sourceUrl}`, 'Usage:');
    await reject(`/audio --height 60 ${sourceUrl}`, 'Usage:');

    // Hold the worker to prove that preferences freeze at admission, not execution.
    await fixtureSql("UPDATE dl_user_settings SET quality = 'best', audio_codec = 'mp3' WHERE user_id = 42");
    sourceFixture.closeGate();
    const blocker = await submit(`/download http://127.0.0.1:${sourcePort}/gated?case=preference-blocker`);
    await waitFor(() => sourceFixture.gatedResponses.size > 0, 'preference snapshot worker blocker');
    const audioCommand = `/audio ${sourceUrl}`;
    const frozen = await submit(audioCommand);
    assert((await queryJob(frozen.operationId)).startsWith('queued|'));
    assert.equal((await request(frozen.operationId)).params.audio_codec, 'mp3');
    await fixtureSql("UPDATE dl_user_settings SET quality = 'bogus', audio_codec = 'invalid' WHERE user_id = 42");
    botApi.setExpectedUpload({ method: 'sendaudio', filename: 'audio.mp3', mimeType: 'audio/mpeg', preserveMdat: false });
    const before = botApi.uploadCalls;
    await cancel(blocker.operationId);
    sourceFixture.releaseGatedResponses();
    await succeeded(frozen.operationId);
    assert.equal(botApi.uploadCalls, before + 1);
    await inspectUpload('frozen saved audio', { video: false, audio: true, audioCodec: 'mp3' });
    await assertWorkspaceClean();
    await replay(audioCommand, frozen);
    await reject(audioCommand, 'Check your download settings before retrying.');
    botApi.setExpectedUpload(null);

    const explicitAudio = await execute('explicit AAC rescues invalid saved codec',
      `/audio --audio-codec aac ${sourceUrl}`, 'sendaudio',
      { video: false, audio: true, audioCodec: 'aac', durationMin: 0.8, durationMax: 1.5 },
      { filename: 'audio.m4a', mimeType: 'audio/mp4' });
    assert.equal((await request(explicitAudio.operationId)).params.audio_codec, 'aac');
    assert.notEqual(await cacheKey(frozen.operationId), await cacheKey(explicitAudio.operationId));
    await reject(`/audio --audio-codec aac ${sourceUrl}`, 'This request conflicts with an existing operation.', frozen.messageId);
    await assertWorkspaceClean();
    return 'saved format preferences, explicit overrides, immutable admission/replay and real codec/height/file delivery passed';
  } finally {
    botApi.setExpectedUpload(null);
    sourceFixture.releaseGatedResponses();
    if (previous) {
      await fixtureSql(`UPDATE dl_user_settings SET ${fields.map(f => `${f} = ${literal(previous[f])}`).join(', ')} WHERE user_id = 42`);
    } else {
      await fixtureSql('DELETE FROM dl_user_settings WHERE user_id = 42');
    }
  }
}
