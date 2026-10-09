import assert from 'node:assert/strict';
import { writeFile, unlink } from 'node:fs/promises';
import path from 'node:path';
import { commandHarness } from './command-harness.mjs';
import { assertCurrentImageHooks, runYoutubeAuthCurrentImage } from './youtube-auth-current-image.mjs';

const literal = value => value === null ? 'NULL' : typeof value === 'number' ? String(value) : `'${String(value).replaceAll("'", "''")}'`;
const access = 'synthetic-access';
const refresh = 'synthetic-refresh';
const observed = 'synthetic-refresh-observed';
const content = (token, expiry = '2099-01-01T00:00:00Z') => '__oauth2__' + JSON.stringify({ access_token: token, refresh_token: refresh, expires_at: expiry });

export function assertYoutubeAuthEvidence(value) {
  const evidence = typeof value === 'string' ? value : JSON.stringify(value);
  for (const secret of [access, refresh, observed, 'synthetic-native-access', 'synthetic-native-refresh', 'yoink-fixture-synthetic-access-not-a-real-token', 'yoink-fixture-synthetic-refresh-not-a-real-token', '__oauth2__']) {
    assert(!evidence.includes(secret), 'synthetic OAuth credential exposed in persisted/reply evidence');
  }
}

export function assertYoutubeAuthRequest(request, cookieId) {
  assert(request.network?.proxy == null, 'OAuth fixture proxy must be disabled');
  const cookie = request.network?.cookie;
  assert(cookie, 'durable selected cookie reference required');
  assert.equal(cookie.id, cookieId);
  assert.equal(cookie.owner_id, 42);
  assert.equal(cookie.domain, 'youtube.com');
  assert.equal(cookie.is_pool, false);
  assert.match(cookie.version, /^[a-f0-9]{64}$/);
  const persisted = JSON.stringify(request);
  assertYoutubeAuthEvidence(persisted);
  assert(!/"(?:youtube_oauth|access_token|refresh_token|expires_at)"/.test(persisted), 'transient OAuth fields must not be persisted');
}

export function assertYoutubeAuthDelivery(call, receipt, sends) {
  // BotApi media calls intentionally have flattened fields, unlike sendmessage.data.
  assert.equal(call?.method, 'sendvideo');
  assert.equal(call.status, 'success');
  assert.equal(call.chat_id, 42);
  assert.equal(call.cached, false);
  assert(call.uploadLength > 100, 'actual multipart upload required');
  assert.equal(receipt.status, 'succeeded');
  assert.equal(receipt.planned, 1);
  assert.equal(receipt.delivered, 1);
  assert.equal(receipt.units.length, 1);
  const unit = receipt.units[0];
  assert.equal(unit.attempt, receipt.attempt);
  assert.equal(unit.ordinal, 0);
  assert.equal(unit.artifact_count, 1);
  assert.equal(unit.status, 'sent');
  // BotApi's actual successful response uses message_id=9000+sendCalls.
  assert.deepEqual(JSON.parse(unit.message_ids), [9000 + sends]);
  assert.equal(receipt.message_id, 9000 + sends);
}

export async function runYoutubeAuthScenarios(context) {
  if (context.currentImageAcceptance) assertCurrentImageHooks(context);
  const { botApi, fixtureSql, waitFor, queryJob, helperContainer, image, fixtureDir, restartApp, assertWorkspaceClean } = context;
  const h = commandHarness(context, 3_100_000);
  const fields = ['youtube_auth_mode', 'proxy_enabled', 'proxy_url', 'use_pool_cookies', 'quality', 'codec', 'container', 'audio_codec', 'send_as_file', 'updated_at'];
  const old = JSON.parse(await fixtureSql(`SELECT json_object(${fields.flatMap(f => [literal(f), f]).join(',')}) FROM dl_user_settings WHERE user_id=42`) || 'null');
  assert.equal(await fixtureSql("SELECT count(*) FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='youtube.com'"), '0', 'fixture needs a fresh personal YouTube credential namespace');
  let cookieId, wroteProbe = false;
  const probeFile = path.join(fixtureDir, 'youtube-auth-upload.mp4');
  try {
    await fixtureSql('INSERT INTO dl_user_settings(user_id,created_at,updated_at) VALUES(42,unixepoch()*1000000,unixepoch()*1000000) ON CONFLICT(user_id) DO NOTHING');
    await fixtureSql("UPDATE dl_user_settings SET youtube_auth_mode='oauth',proxy_enabled=0,proxy_url=NULL,use_pool_cookies=0,quality='best',codec='avc1',container='mp4',audio_codec='best',send_as_file=0 WHERE user_id=42");
    cookieId = Number(await fixtureSql(`INSERT INTO cookies(user_id,domain,content,is_valid,is_pool,created_at,updated_at) VALUES(42,'youtube.com',${literal(content(access))},1,0,unixepoch()*1000000,unixepoch()*1000000) RETURNING id`));
    assert(Number.isSafeInteger(cookieId) && cookieId > 0);
    botApi.setExpectedUpload({ method: 'sendvideo', filename: 'media.mp4', mimeType: 'video/mp4', preserveMdat: false });
    const command = '/download https://youtu.be/Synthetic01';
    const beforeUploads = botApi.uploadCalls, beforeSends = botApi.sendCalls;
    const sent = await h.submit(command);
    const request = await h.request(sent.operationId);
    assertYoutubeAuthRequest(request, cookieId);
    await h.succeeded(sent.operationId);
    assert.equal(botApi.uploadCalls, beforeUploads + 1);
    assert.equal(botApi.sendCalls, beforeSends + 1);
    const calls = botApi.calls.slice(sent.callStart).filter(c => ['sendvideo', 'senddocument', 'sendmediagroup', 'sendaudio', 'sendphoto'].includes(c.method));
    assert.equal(calls.length, 1);
    const receiptSql = `SELECT json_object('status',status,'attempt',attempt,'planned',planned_artifact_count,'delivered',delivered_artifact_count,'message_id',delivered_message_id,'units',json((SELECT json_group_array(json_object('attempt',attempt,'ordinal',ordinal,'artifact_count',artifact_count,'status',status,'message_ids',message_ids)) FROM runtime_delivery_units WHERE operation_id=runtime_operations.id))) FROM runtime_operations WHERE id=${literal(sent.operationId)}`;
    const receipt = await fixtureSql(receiptSql);
    assertYoutubeAuthEvidence(receipt);
    assertYoutubeAuthEvidence(botApi.calls.slice(sent.callStart));
    assertYoutubeAuthDelivery(calls[0], JSON.parse(receipt), botApi.sendCalls);
    const bytes = botApi.lastUploadedBytes;
    assert(Buffer.isBuffer(bytes) && bytes.length > 100 && bytes.length < 1024 * 1024);
    await writeFile(probeFile, bytes, { flag: 'wx', mode: 0o444 });
    wroteProbe = true;
    const probe = JSON.parse(await helperContainer(['--network', 'none', '--mount', `type=bind,source=${fixtureDir},target=/fixture,readonly`, '--entrypoint', 'ffprobe', image, '-v', 'error', '-show_entries', 'format=duration:stream=codec_type,codec_name,width,height', '-of', 'json', '/fixture/youtube-auth-upload.mp4'], 15000));
    assert(probe.streams.some(s => s.codec_type === 'video' && s.codec_name === 'h264' && s.width === 160 && s.height === 90));
    assert(probe.streams.some(s => s.codec_type === 'audio' && s.codec_name === 'aac'));
    assert(Number(probe.format.duration) >= 0.8 && Number(probe.format.duration) <= 1.5);
    await restartApp();
    await h.replay(command, sent);
    const replayRequest = await h.request(sent.operationId);
    assertYoutubeAuthRequest(replayRequest, cookieId);
    assert.deepEqual(replayRequest, request);
    const replayReceipt = await fixtureSql(receiptSql);
    assertYoutubeAuthEvidence(replayReceipt);
    assert.equal(replayReceipt, receipt, 'replay must preserve receipt/journal');
    await fixtureSql(`UPDATE cookies SET content=${literal(content(observed))},updated_at=updated_at+1 WHERE id=${cookieId}`);
    const rejected = await h.submit('/download https://youtu.be/Synthetic02');
    await waitFor(async () => (await queryJob(rejected.operationId)).startsWith('failed|'), 'observed upstream refresh rejected', 45000);
    const rejectedRequest = await h.request(rejected.operationId);
    assertYoutubeAuthRequest(rejectedRequest, cookieId);
    assert.notEqual(rejectedRequest.network.cookie.version, request.network.cookie.version, 'changed credential requires a fresh reference');
    assert.equal(botApi.uploadCalls, beforeUploads + 1);
    assert.equal(botApi.sendCalls, beforeSends + 1);
    assert.equal(await fixtureSql(`SELECT count(*) FROM runtime_delivery_units WHERE operation_id=${literal(rejected.operationId)}`), '0');
    assert.equal(await fixtureSql(`SELECT count(*) FROM runtime_media_cache WHERE cache_key=(SELECT cache_key FROM runtime_operations WHERE id=${literal(rejected.operationId)})`), '0');
    assert((await fixtureSql(`SELECT content FROM cookies WHERE id=${cookieId}`)) === content(observed), 'no Google refresh or credential writeback');
    // Inspect operation/journal and public replies, but never include the cookie table:
    // its transient credential is intentionally stored there and nowhere else.
    assertYoutubeAuthEvidence(await fixtureSql(`SELECT * FROM runtime_operations WHERE id IN (${literal(sent.operationId)},${literal(rejected.operationId)})`));
    assertYoutubeAuthEvidence(await fixtureSql(`SELECT * FROM runtime_delivery_units WHERE operation_id IN (${literal(sent.operationId)},${literal(rejected.operationId)})`));
    assertYoutubeAuthEvidence(botApi.calls.slice(sent.callStart));
    assert(!botApi.error, 'BotApi fixture reported an error');
    await assertWorkspaceClean();
    if (context.currentImageAcceptance) {
      const combined = await runYoutubeAuthCurrentImage(context, {
        cookieId, credential: expiry => content(access, expiry), assertEvidence: assertYoutubeAuthEvidence, assertRequest: assertYoutubeAuthRequest,
      });
      return `synthetic canonical OAuth multipart receipt/replay/rotation rejection; ${combined} (not live-provider or real PoToken issuance)`;
    }
    return 'deterministic synthetic native YouTube OAuth multipart delivery, frozen reference, receipt/journal, restart replay and observed-refresh rejection (not real-provider or refresh-admission acceptance)';
  } finally {
    botApi.setExpectedUpload(null);
    if (cookieId) await fixtureSql(`DELETE FROM cookies WHERE id=${cookieId}`);
    if (old) await fixtureSql(`UPDATE dl_user_settings SET ${Object.entries(old).map(([k, v]) => `${k}=${literal(v)}`).join(',')} WHERE user_id=42`);
    else await fixtureSql('DELETE FROM dl_user_settings WHERE user_id=42');
    if (wroteProbe) await unlink(probeFile).catch(error => { if (error.code !== 'ENOENT') throw error; });
  }
}
