import assert from 'node:assert/strict';
import { commandHarness } from './command-harness.mjs';
import { subtitleBytes } from './subtitle-source.mjs';

export async function runSubtitleScenarios(context) {
  const { botApi, fixtureSql, sourcePort, sourceFixture, waitFor, queryJob, restartApp, assertWorkspaceClean } = context;
  const h = commandHarness(context, 2900);
  const fields = ['subs_lang', 'subs_auto', 'subs_enabled'];
  const old = await fixtureSql(`SELECT json_object(${fields.map(f => `'${f}',${f}`).join(',')}) FROM dl_user_settings WHERE user_id=42`);
  const previous = old.trim() ? JSON.parse(old) : null;
  const literal = value => typeof value === 'number' ? String(value) : `'${value.replaceAll("'", "''")}'`;
  const url = params => `http://127.0.0.1:${sourcePort}/subtitles.html?${new URLSearchParams(params)}`;
  const terminal = (id, state) => waitFor(async () => (await queryJob(id)).startsWith(`${state}|`), `subtitle ${state}`, 45000);
  const document = (code, extension = 'vtt') => ({ method: 'senddocument', filename: `subtitles-${code}.${extension}`, mimeType: extension === 'vtt' ? 'text/vtt' : 'application/x-subrip', preserveMdat: false });
  try {
    await fixtureSql("INSERT INTO dl_user_settings (user_id, quality, codec, container, proxy_enabled, keyboard, subs_enabled, subs_auto, subs_always_ask, subs_lang, split_size, nsfw_blur, mediainfo, send_as_file, audio_codec, gallery_zip, use_pool_cookies, youtube_auth_mode, args_json, created_at, updated_at) VALUES (42, 'best', 'avc1', 'mp4', 0, '2x3', 0, 0, 0, 'en', 2043000000, 1, 0, 0, 'best', 0, 1, 'cookies', '{}', unixepoch('now')*1000000, unixepoch('now')*1000000) ON CONFLICT(user_id) DO UPDATE SET subs_enabled=0, subs_auto=0, subs_lang='en'");
    botApi.setExpectedUpload(document('en'));
    const command = `/subs ${url({ case: 'manual' })}`;
    const uploads = botApi.uploadCalls;
    const first = await h.submit(command);
    await h.succeeded(first.operationId);
    assert.equal(botApi.uploadCalls, uploads + 1);
    assert.deepEqual(botApi.lastUploadedBytes, subtitleBytes('en'));
    assert.deepEqual((await h.request(first.operationId)).params.subtitles, { language: 'en', automatic: false });
    await h.replay(command, first);
    let fetched = sourceFixture.mediaRequests, cached = botApi.cachedCalls;
    const reused = await h.submit(command);
    await h.succeeded(reused.operationId);
    assert.equal(sourceFixture.mediaRequests, fetched);
    assert.equal(botApi.cachedCalls, cached + 1);
    botApi.setSendMode('invalid-file-id-once');
    const recovered = await h.submit(command);
    await h.succeeded(recovered.operationId);
    assert(sourceFixture.mediaRequests > fetched);
    assert.equal(botApi.uploadCalls, uploads + 2);
    assert.deepEqual(botApi.lastUploadedBytes, subtitleBytes('en'));

    await fixtureSql("UPDATE dl_user_settings SET subs_lang='ru', subs_auto=1 WHERE user_id=42");
    botApi.setExpectedUpload(document('ru'));
    const russian = await h.submit(command);
    await h.succeeded(russian.operationId);
    assert.deepEqual(botApi.lastUploadedBytes, subtitleBytes('ru'));
    assert.deepEqual((await h.request(russian.operationId)).params.subtitles, { language: 'ru', automatic: true });
    const beforeOverride = botApi.uploadCalls;
    botApi.setExpectedUpload(document('en'));
    const override = await h.submit(`${command} --subtitle-language en --subtitle-auto false`);
    await h.succeeded(override.operationId);
    assert.equal(botApi.uploadCalls, beforeOverride, 'effective subtitle selection, not flag spelling, keys cache');
    assert.deepEqual((await h.request(override.operationId)).params.subtitles, { language: 'en', automatic: false });
    fetched = sourceFixture.mediaRequests;
    const automatic = await h.submit(`${command} --subtitle-language en --subtitle-auto true`);
    await h.succeeded(automatic.operationId);
    assert(sourceFixture.mediaRequests > fetched, 'automatic selection cannot reuse manual-only cache');

    botApi.setExpectedUpload(document('en', 'srt'));
    const srt = await h.submit(`/subs ${url({ format: 'srt' })} --subtitle-language en`);
    await h.succeeded(srt.operationId);
    assert.deepEqual(botApi.lastUploadedBytes, subtitleBytes('en', 'srt'));
    botApi.setExpectedUpload(null);
    botApi.setExpectedAlbum(['ru', 'ru-orig'].map(code => ({ kind: 'document', filename: `subtitles-${code}.vtt`, mimeType: 'text/vtt', bytes: subtitleBytes(code) })));
    const pair = await h.submit(`/subs ${url({ tracks: 'ru,ru-orig,en' })} --subtitle-language ru`);
    await h.succeeded(pair.operationId);
    assert.deepEqual(botApi.lastGroupBytes, ['ru', 'ru-orig'].map(code => subtitleBytes(code)));
    botApi.setExpectedAlbum(null);
    assert.equal(sourceFixture.subtitleVideoRequests, 0, 'subtitle-only operations must never fetch video bytes');
    await assertWorkspaceClean();

    await restartApp();
    fetched = sourceFixture.mediaRequests; cached = botApi.cachedCalls;
    const afterRestart = await h.submit(`${command} --subtitle-language en --subtitle-auto false`);
    await h.succeeded(afterRestart.operationId);
    assert.equal(sourceFixture.mediaRequests, fetched);
    assert.equal(botApi.cachedCalls, cached + 1);
    for (const params of [{ tracks: 'fr' }, { tracks: 'ru,ru-orig', case: 'missing' }, { case: 'invalid' }, { case: 'oversized' }, { case: 'private-redirect' }]) {
      const sends = botApi.sendCalls;
      const failed = await h.submit(`/subs ${url(params)} --subtitle-language ${params.case === 'missing' ? 'ru' : 'en'}`);
      await terminal(failed.operationId, 'failed');
      assert.equal(botApi.sendCalls, sends, 'failed subtitle extraction cannot deliver a partial batch');
      assert.equal(await fixtureSql(`SELECT count(*) FROM runtime_delivery_units WHERE operation_id='${failed.operationId}'`), '0');
      await assertWorkspaceClean();
    }
    await h.reject('/subs invalid-input', 'Usage:');
    await h.reject(`${command} --height 720`, 'Usage:');
    await h.reject(`${command} --subtitle-language en.*`, 'Usage:');
    await fixtureSql("UPDATE dl_user_settings SET subs_lang='en.*' WHERE user_id=42");
    await h.reject(command, 'Check your download settings');
    await h.replay(command, first);
    return 'standalone VTT/SRT and original-track documents, saved language/automatic snapshots, explicit overrides, cache partition/recovery/restart, no video downloads, missing/invalid/oversized/private-redirect rejection without partial delivery';
  } finally {
    botApi.setExpectedUpload(null); botApi.setExpectedAlbum(null); botApi.setSendMode('success');
    if (previous) await fixtureSql(`UPDATE dl_user_settings SET ${fields.map(f => `${f}=${literal(previous[f])}`).join(',')} WHERE user_id=42`);
    else await fixtureSql('DELETE FROM dl_user_settings WHERE user_id=42');
  }
}
