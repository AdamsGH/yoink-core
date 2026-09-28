import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { commandHarness } from './command-harness.mjs';
import { galleryImage } from './gallery-source.mjs';

export async function runGalleryScenarios(context) {
  const { botApi, fixtureSql, queryJob, waitFor, assertWorkspaceClean, gallerySource, proxyUrl, restartGallery, restartApp, helperContainer, image } = context;
  const h = commandHarness(context, 2600);
  const fields = ['proxy_enabled', 'proxy_url', 'send_as_file', 'gallery_zip'];
  const old = await fixtureSql(`SELECT json_object(${fields.map(f => `'${f}',${f}`).join(',')}) FROM dl_user_settings WHERE user_id=42`);
  const previous = old.trim() ? JSON.parse(old) : null;
  const literal = value => value === null ? 'NULL' : typeof value === 'number' ? String(value) : `'${value.replaceAll("'", "''")}'`;
  const album = (count, kind = 'photo') => Array.from({ length: count }, (_, i) => ({ kind, filename: 'image.png', mimeType: 'image/png', bytes: galleryImage(i + 1) }));
  const counts = id => fixtureSql(`SELECT planned_artifact_count || '|' || delivered_artifact_count FROM runtime_operations WHERE id='${id}'`);
  const journal = id => fixtureSql(`SELECT ordinal || '|' || artifact_count || '|' || status || '|' || COALESCE(json_array_length(message_ids),0) FROM runtime_delivery_units WHERE operation_id='${id}' ORDER BY ordinal`);
  const terminal = (id, state) => waitFor(async () => (await queryJob(id)).startsWith(`${state}|`), `gallery ${state}`, 45000);
  const source = key => `https://imgbox.com/g/${key}`;
  try {
    await fixtureSql(`INSERT INTO dl_user_settings (user_id, quality, codec, container, proxy_enabled, proxy_url, keyboard, subs_enabled, subs_auto, subs_always_ask, subs_lang, split_size, nsfw_blur, mediainfo, send_as_file, audio_codec, gallery_zip, use_pool_cookies, youtube_auth_mode, args_json, created_at, updated_at) VALUES (42, 'best', 'avc1', 'mp4', 1, ${literal(proxyUrl)}, '2x3', 0, 0, 0, 'en', 2043000000, 1, 0, 0, 'best', 0, 1, 'cookies', '{}', unixepoch('now')*1000000, unixepoch('now')*1000000) ON CONFLICT(user_id) DO UPDATE SET proxy_enabled=1, proxy_url=${literal(proxyUrl)}, send_as_file=0, gallery_zip=0`);
    await restartGallery(true, false);
    const untrustedSends = botApi.sendCalls;
    const untrusted = await h.submit(`/image ${source('Gallery001')}`);
    await terminal(untrusted.operationId, 'failed');
    assert.equal(gallerySource.requests, 0, 'untrusted TLS must fail before an HTTP request');
    assert.equal(botApi.sendCalls, untrustedSends);
    await assertWorkspaceClean();
    await restartGallery(true);
    botApi.setExpectedAlbum(album(3));
    const command = `/image ${source('Gallery003')}`;
    const before = botApi.uploadCalls;
    const first = await h.submit(command);
    await h.succeeded(first.operationId);
    assert.equal(botApi.uploadCalls, before + 1);
    assert.deepEqual(botApi.lastGroupBytes, album(3).map(item => item.bytes));
    assert.equal(await counts(first.operationId), '3|3');
    assert.equal(await journal(first.operationId), '0|3|sent|3');
    assert(gallerySource.requests >= 7, 'gallery page, three image pages and three files must be extracted');
    await h.replay(command, first);
    const fetched = gallerySource.requests, cachedBefore = botApi.cachedCalls;
    const cached = await h.submit(command);
    await h.succeeded(cached.operationId);
    assert.equal(gallerySource.requests, fetched);
    assert.equal(botApi.cachedCalls, cachedBefore + 1);
    assert.equal(botApi.uploadCalls, before + 1);

    botApi.setSendMode('invalid-file-id-once');
    const recovered = await h.submit(command);
    await h.succeeded(recovered.operationId);
    assert(gallerySource.requests > fetched);
    assert.equal(botApi.uploadCalls, before + 2);

    botApi.setExpectedAlbum(album(3, 'document'));
    const documents = await h.submit(`/image --send-as-file true ${source('Gallery003')}`);
    await h.succeeded(documents.operationId);
    assert.equal(await journal(documents.operationId), '0|3|sent|3');

    botApi.setExpectedAlbum(album(9));
    botApi.setExpectedUpload({ method: 'senddocument', filename: 'archive.zip', mimeType: 'application/zip', preserveMdat: false });
    const zipCommand = `/image --gallery-zip true ${source('Gallery012')}`;
    const zipped = await h.submit(zipCommand);
    await h.succeeded(zipped.operationId);
    assert.equal(await counts(zipped.operationId), '10|10');
    assert.equal(await journal(zipped.operationId), '0|9|sent|9\n1|1|sent|1');
    const archive = JSON.parse(await helperContainer(['--network', 'none', '--entrypoint', 'python', image, '-I', '-c',
      'import hashlib,io,json,sys,zipfile; z=zipfile.ZipFile(io.BytesIO(sys.stdin.buffer.read())); print(json.dumps([[n,hashlib.sha256(z.read(n)).hexdigest()] for n in z.namelist()]))'], 15000, botApi.lastUploadedBytes));
    assert.deepEqual(archive, Array.from({ length: 12 }, (_, i) => [`${String(i + 1).padStart(4, '0')}.png`, createHash('sha256').update(galleryImage(i + 1)).digest('hex')]));
    const zipFetches = gallerySource.requests, zipSends = botApi.cachedCalls;
    const cachedZip = await h.submit(zipCommand);
    await h.succeeded(cachedZip.operationId);
    assert.equal(gallerySource.requests, zipFetches);
    assert.equal(botApi.cachedCalls, zipSends + 2, 'photo album and ZIP need separate cached sends');

    botApi.setSendPlan(['success', 'api-error']);
    const partialCommand = `/image --gallery-zip true ${source('Gallery011')}`;
    const partial = await h.submit(partialCommand);
    await terminal(partial.operationId, 'delivery_partial');
    assert.equal(await counts(partial.operationId), '10|9');
    assert.equal(await journal(partial.operationId), '0|9|sent|9\n1|1|rejected|0');
    botApi.setSendMode('success');
    await assertWorkspaceClean();
    await restartApp();
    await h.replay(partialCommand, partial);
    assert.equal(await counts(partial.operationId), '10|9');

    botApi.setExpectedAlbum(null); botApi.setExpectedUpload(null);
    const sends = botApi.sendCalls;
    const broken = await h.submit(`/image ${source('Missing003')}`);
    await terminal(broken.operationId, 'failed');
    assert.equal(botApi.sendCalls, sends, 'failed source item cannot deliver its downloaded prefix');
    assert.equal(await journal(broken.operationId), '');
    assert(gallerySource.paths.has('/file-Missing003-3.png'), 'negative source case must reach its missing image');
    const limited = await h.submit(`/image ${source('Gallery021')}`);
    await terminal(limited.operationId, 'failed');
    assert.equal(botApi.sendCalls, sends, 'source item budget failure cannot send any album');
    assert(gallerySource.paths.has('/file-Gallery021-20.png'), 'item limit case must first download twenty valid images');
    await assertWorkspaceClean();
  } finally {
    botApi.setSendMode('success'); botApi.setSendPlan([]); botApi.setExpectedAlbum(null); botApi.setExpectedUpload(null);
    if (previous) await fixtureSql(`UPDATE dl_user_settings SET ${fields.map(f => `${f}=${literal(previous[f])}`).join(',')} WHERE user_id=42`);
    else await fixtureSql('DELETE FROM dl_user_settings WHERE user_id=42');
    await restartGallery(false);
  }
  return 'pinned gallery extraction over local verified TLS, ordered photo/document albums, ZIP bytes/cache, partial delivery/restart and source failure limits passed';
}
