import assert from 'node:assert/strict';
import { commandHarness } from './command-harness.mjs';

export function parseFormatReport(bytes) {
  assert(Buffer.isBuffer(bytes) && bytes.length > 0 && bytes.length <= 65536);
  const report = bytes.toString('utf8');
  assert.equal(report.split('\n').slice(0, 2).join('\n'), 'Yoink source formats v1\nID\tEXT\tWIDTH\tHEIGHT\tFPS\tVIDEO\tAUDIO\tPROTOCOL');
  assert(report.endsWith('\n'));
  const rows = report.split('\n').slice(2, -1).map(line => line.split('\t'));
  assert(rows.length >= 1 && rows.length <= 100);
  for (const row of rows) {
    assert.equal(row.length, 8);
    assert.match(row[0], /^(?:\d{1,10}|-)$/);
    assert.match(row[1], /^(?:mp4|webm|m4a|mp3|ogg|opus|flv|mkv|unknown)$/);
    for (const value of row.slice(2, 4)) assert(value === '-' || /^\d+$/.test(value) && Number(value) <= 16384);
    assert(row[4] === '-' || /^\d+(?:\.\d{1,3})?$/.test(row[4]) && Number(row[4]) <= 1000);
    assert.match(row[5], /^(?:h264|h265|av1|vp9|vp8|none|unknown)$/);
    assert.match(row[6], /^(?:aac|mp3|opus|vorbis|flac|none|unknown)$/);
    assert.match(row[7], /^(?:https|http|hls|dash|unknown)$/);
  }
  return rows;
}

export async function runFormatScenarios(context) {
  const { botApi, sourceFixture, sourcePort, proxyUrl, fixtureSql, queryJob, waitFor, restartApp, assertWorkspaceClean, cookieSecret } = context;
  const h = commandHarness(context, 46000);
  const source = sourceFixture.formatSource;
  const root = `http://127.0.0.1:${sourcePort}/format-fixture`;
  const url = `${root}/master.m3u8?key=format-url-secret`;
  const upload = { method: 'senddocument', filename: 'formats.txt', mimeType: 'text/plain', preserveMdat: false };
  const literal = value => value === null ? 'NULL' : typeof value === 'number' ? String(value) : `'${String(value).replaceAll("'", "''")}'`;
  const fields = ['quality', 'codec', 'container', 'audio_codec', 'subs_lang', 'send_as_file', 'proxy_enabled', 'proxy_url', 'updated_at'];
  const selectSettings = `SELECT json_object(${fields.map(f => `'${f}',${f}`).join(',')}) FROM dl_user_settings WHERE user_id=42`;
  const oldSettingsText = await fixtureSql(selectSettings);
  const oldSettings = oldSettingsText.trim() ? JSON.parse(oldSettingsText) : null;
  const oldUser = JSON.parse(await fixtureSql("SELECT json_object('role',role,'ban_until',ban_until,'language',language,'updated_at',updated_at) FROM users WHERE id=42"));
  const cookieFields = ['id', 'user_id', 'domain', 'content', 'is_valid', 'is_pool', 'created_at', 'updated_at', 'validated_at', 'label', 'avatar_url', 'content_hash', 'session_key'];
  const oldCookies = JSON.parse(await fixtureSql(`SELECT json_group_array(json_object(${cookieFields.map(f => `'${f}',${f}`).join(',')})) FROM cookies WHERE user_id=42 AND domain='127.0.0.1' AND is_pool=0`));
  const count = () => fixtureSql('SELECT count(*) FROM runtime_operations');
  let cacheTrapKey = null;
  const noPayloads = () => assert(source.requests.every(r => (/\.(m3u8|html)$/.test(r.path) || r.path.endsWith('/direct.mp4'))), 'metadata probe must never request media payloads or playlist children');
  const terminal = id => waitFor(async () => {
    const row = await queryJob(id);
    return /^(failed|succeeded|delivery_unknown|partially_delivered)\|/.test(row) ? row : false;
  }, 'format report terminal', 45000);
  async function report(command, options = {}) {
    botApi.setExpectedUpload(upload);
    const before = botApi.uploadCalls;
    const sent = await h.submit(command, options);
    await h.succeeded(sent.operationId);
    assert.equal(botApi.uploadCalls, before + 1);
    const rows = parseFormatReport(botApi.lastUploadedBytes);
    if (command.includes('/direct.mp4')) assert(rows.some(row => row[1] === 'mp4' && row[5] === 'unknown' && row[6] === 'unknown' && row[7] === 'http'));
    else assert(rows.some(row => row[2] === '1280' && row[3] === '720' && row[5] === 'h264' && row[6] === 'aac' && row[7] === 'hls'));
    const text = botApi.lastUploadedBytes.toString();
    for (const secret of ['format-url-secret', 'manifest-secret', 'payload-secret', 'metadata-title-secret', cookieSecret, proxyUrl]) assert(!text.includes(secret));
    assert(!text.includes('http://') && !text.includes('https://'));
    const request = await h.request(sent.operationId);
    assert.equal(request.media_mode, 'formats');
    assert.equal(request.params.send_as_file, true);
    assert.equal(request.params.max_height, null);
    assert(!request.params.subtitles);
    assert(!request.clip_range && !request.params.playlist);
    assert.equal(await fixtureSql(`SELECT count(*) FROM runtime_media_cache WHERE actor_id=42 AND bot_id=123456 AND cache_key IN (SELECT cache_key FROM runtime_operations WHERE actor_id=42 AND json_extract(request,'$.source_url')=${literal(request.source_url)})`), cacheTrapKey && request.source_url === url ? '1' : '0');
    noPayloads();
    await assertWorkspaceClean();
    return sent;
  }
  try {
    await fixtureSql("INSERT INTO dl_user_settings(user_id,created_at,updated_at) VALUES(42,unixepoch('now')*1000000,unixepoch('now')*1000000) ON CONFLICT(user_id) DO NOTHING");
    await fixtureSql("UPDATE dl_user_settings SET quality='ask',codec='invalid',container='invalid',audio_codec='invalid',subs_lang='invalid',send_as_file=0,proxy_enabled=0 WHERE user_id=42");
    const preferencesBefore = await fixtureSql(selectSettings);
    const first = await report(`/list ${url}`);
    const reads = source.requests.length;
    await h.replay(`/list ${url}`, first);
    await restartApp();
    await h.replay(`/list ${url}`, first);
    assert.equal(source.requests.length, reads, 'replay must not rerun metadata extraction');
    cacheTrapKey = await fixtureSql(`SELECT cache_key FROM runtime_operations WHERE id='${first.operationId}'`);
    const trapMedia = JSON.stringify([{ kind: 'document', file_id: 'forbidden_format_cache_id', size_bytes: 512 }]);
    await fixtureSql(`INSERT INTO runtime_media_cache(actor_id,bot_id,cache_key,revision,media,expires_at) VALUES(42,123456,${literal(cacheTrapKey)},'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',${literal(trapMedia)},unixepoch('now')*1000000+600000000)`);
    const cachedBefore = botApi.cachedCalls;
    const fresh = await report(`/list@YOINK_FIXTURE_BOT ${url}`);
    assert.equal(botApi.cachedCalls, cachedBefore, 'a valid matching cache entry must not replace source-format discovery');
    assert.equal(await fixtureSql(`SELECT media FROM runtime_media_cache WHERE actor_id=42 AND bot_id=123456 AND cache_key=${literal(cacheTrapKey)}`), trapMedia, 'report processing must leave existing media cache untouched');
    await fixtureSql(`DELETE FROM runtime_media_cache WHERE actor_id=42 AND bot_id=123456 AND cache_key=${literal(cacheTrapKey)}`);
    cacheTrapKey = null;
    assert.notEqual(fresh.operationId, first.operationId);
    assert(source.requests.length > reads, 'new message must get fresh metadata rather than use media cache');
    await report(`/list ${root}/direct.mp4`);
    assert.equal(await fixtureSql(selectSettings), preferencesBefore, 'format discovery cannot modify saved settings');

    const beforeChange = await count();
    const changed = botApi.calls.length;
    botApi.enqueue(42, 42, `/list ${root}/master.m3u8?other=intent`, first.messageId);
    await waitFor(() => botApi.calls.slice(changed).some(c => c.method === 'sendmessage'), 'changed format intent rejected');
    assert.equal(await count(), beforeChange);
    assert.equal(source.requests.length > reads, true);
    for (const invalid of ['/list', `/list ${url} extra`, `/list --height 720 ${url}`, '/list https://user:password@example.test/media', '/list file:///etc/passwd']) {
      const from = botApi.calls.length, operations = await count();
      botApi.enqueue(42, 42, invalid);
      await waitFor(() => botApi.calls.slice(from).some(c => c.method === 'sendmessage'), 'invalid list usage');
      assert.equal(await count(), operations);
    }
    const silence = botApi.calls.length, operations = await count(), requests = source.requests.length;
    botApi.enqueue(43, 42, `/list ${url}`);
    botApi.enqueue(42, -10042, `/list ${url}`);
    botApi.enqueue(42, 42, `/list@foreign_bot ${url}`);
    // A supported barrier guarantees the preceding updates were dispatched.
    botApi.enqueue(42, 42, '/status');
    await waitFor(() => botApi.calls.slice(silence).some(c => c.method === 'sendmessage'), 'format authorization barrier');
    assert.equal(await count(), operations);
    assert.equal(source.requests.length, requests);
    assert(botApi.calls.slice(silence).every(c => c.data.chat_id === 42));

    const reportSends = botApi.uploadCalls;
    for (const failed of [`${root}/redirect.m3u8`, `${root}/playlist.html`, `${root}/missing.m3u8`]) {
      const sent = await h.submit(`/list ${failed}`);
      assert.match(await terminal(sent.operationId), /^failed\|/);
      assert.equal(botApi.uploadCalls, reportSends);
      assert.equal(await fixtureSql(`SELECT count(*) FROM runtime_delivery_units WHERE operation_id='${sent.operationId}'`), '0');
      await assertWorkspaceClean();
      noPayloads();
    }

    await fixtureSql("DELETE FROM cookies WHERE user_id=42 AND domain='127.0.0.1' AND is_pool=0");
    const jar = `# Netscape HTTP Cookie File\n127.0.0.1\tFALSE\t/\tFALSE\t${Math.floor(Date.now()/1000)+3600}\tsession\t${cookieSecret}\n`;
    await fixtureSql(`INSERT INTO cookies(user_id,domain,content,is_valid,is_pool,created_at,updated_at) VALUES(42,'127.0.0.1',${literal(jar)},1,0,unixepoch('now')*1000000,unixepoch('now')*1000000)`);
    await fixtureSql(`UPDATE dl_user_settings SET proxy_enabled=1,proxy_url=${literal(proxyUrl)} WHERE user_id=42`);
    const network = await report(`/list ${root}/protected/master.m3u8`);
    const selected = await h.request(network.operationId);
    assert(selected.network.cookie && selected.network.proxy, 'format discovery must freeze configured credentials');

    await fixtureSql('UPDATE dl_user_settings SET proxy_enabled=0 WHERE user_id=42');
    const pending = await h.submit(`/list ${root}/master.m3u8?gate=true`);
    await waitFor(() => source.gated.size > 0, 'metadata response gated');
    const sends = botApi.uploadCalls;
    await fixtureSql("UPDATE users SET ban_until=unixepoch('now')*1000000+600000000 WHERE id=42");
    source.release();
    assert.match(await terminal(pending.operationId), /^failed\|/);
    assert.equal(botApi.uploadCalls, sends, 'fresh authorization must deny report delivery after ban');
    const from = botApi.calls.length, before = await count(), sourceBefore = source.requests.length;
    botApi.enqueue(42, 42, `/list ${url}`);
    await waitFor(() => botApi.calls.slice(from).some(c => c.method === 'sendmessage'), 'banned format command denied');
    assert.equal(await count(), before);
    assert.equal(source.requests.length, sourceBefore);
    noPayloads();
  } finally {
    source.release();
    if (cacheTrapKey) await fixtureSql(`DELETE FROM runtime_media_cache WHERE actor_id=42 AND bot_id=123456 AND cache_key=${literal(cacheTrapKey)}`);
    botApi.setExpectedUpload(null);
    await fixtureSql(`UPDATE users SET ${Object.entries(oldUser).map(([f,v]) => `${f}=${literal(v)}`).join(',')} WHERE id=42`);
    if (oldSettings) await fixtureSql(`UPDATE dl_user_settings SET ${Object.entries(oldSettings).map(([f,v]) => `${f}=${literal(v)}`).join(',')} WHERE user_id=42`);
    else await fixtureSql('DELETE FROM dl_user_settings WHERE user_id=42');
    await fixtureSql("DELETE FROM cookies WHERE user_id=42 AND domain='127.0.0.1' AND is_pool=0");
    for (const cookie of oldCookies) await fixtureSql(`INSERT INTO cookies(${cookieFields.join(',')}) VALUES(${cookieFields.map(f => literal(cookie[f])).join(',')})`);
    assert.equal(await fixtureSql(selectSettings), oldSettingsText);
    await assertWorkspaceClean();
  }
  return 'owner-private source-format documents, guarded metadata-only extraction, report redaction, saved cookie/proxy selection, no media cache, strict grammar, authorization, restart replay and failed-probe isolation passed';
}
