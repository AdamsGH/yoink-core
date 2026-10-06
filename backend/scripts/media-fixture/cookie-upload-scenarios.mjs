import assert from 'node:assert/strict';
import { setTimeout as sleep } from 'node:timers/promises';
import { commandHarness } from './command-harness.mjs';
import { withRussianPreference } from './language-fixture.mjs';

export async function runCookieUploadScenarios(context) {
  const { botApi, fixtureSql, waitFor, restartApp, sourceFixture, sourcePort, cookieSecret, queryJob, assertWorkspaceClean } = context;
  const literal = value => value === null ? 'NULL' : typeof value === 'number' ? String(value) : `'${String(value).replaceAll("'", "''")}'`;
  const columns = ['id', 'user_id', 'domain', 'content', 'is_valid', 'created_at', 'updated_at', 'validated_at', 'is_pool', 'label', 'avatar_url', 'content_hash', 'session_key'];
  const savedCookies = await fixtureSql('SELECT * FROM cookies ORDER BY id');
  const savedPersonal = JSON.parse(await fixtureSql(`SELECT json_group_array(json_object(${columns.flatMap(name => [literal(name), name]).join(',')})) FROM (SELECT * FROM cookies WHERE user_id=42 AND is_pool=0 ORDER BY id)`));
  const savedUser = JSON.parse(await fixtureSql("SELECT json_object('role',role,'ban_until',ban_until) FROM users WHERE id=42"));
  const savedSettings = await fixtureSql('SELECT * FROM dl_user_settings WHERE user_id=42');
  const savedWatermark = await fixtureSql('SELECT * FROM runtime_personal_cookie_mutation_watermarks WHERE actor_id=42');
  const savedSettingsObject = JSON.parse(await fixtureSql("SELECT json_object('proxy_enabled',proxy_enabled,'quality',quality,'use_pool_cookies',use_pool_cookies,'updated_at',updated_at) FROM dl_user_settings WHERE user_id=42") || 'null');
  const untouched = await fixtureSql('SELECT * FROM cookies WHERE user_id<>42 OR is_pool=1 ORDER BY id');
  const startCalls = botApi.calls.length;
  const initialOperations = await fixtureSql('SELECT count(*) FROM runtime_operations');
  const cookie = (value = cookieSecret) => `# Netscape HTTP Cookie File\n.example.test\tTRUE\t/\tTRUE\t4102444800\tsessionid\t${value}\n# second line\n#HttpOnly_.example.test\tTRUE\t/\tFALSE\t4102444800\tHttpOnly_auth\t${value}\n`;
  let messageId = 1_000_000;
  let serial = 0;
  const fileId = () => `cookie-fixture-${++serial}`;
  async function upload(bytes, { name = `domain${serial}.test.txt`, domain, language = 'en', mode, message = messageId++, ...route } = {}) {
    const id = fileId();
    botApi.setDocumentFile(id, bytes, { ...(mode?.declaredSize === undefined ? {} : { declaredSize: mode.declaredSize }), ...(mode ? { getFileMode: mode.getFileMode, fileMode: mode.fileMode } : {}), ...((mode?.filePath || mode?.localRemotePath) ? { filePath: mode.filePath ?? mode.localRemotePath } : {}), ...(mode?.delayMs ? { delayMs: mode.delayMs } : {}) });
    const beforeRows = await fixtureSql('SELECT * FROM cookies ORDER BY id');
    const beforeOps = await fixtureSql('SELECT count(*) FROM runtime_operations');
    const beforeWatermark = await fixtureSql('SELECT * FROM runtime_personal_cookie_mutation_watermarks WHERE actor_id=42');
    const beforeDownloads = botApi.downloadCalls.length;
    const beforeCalls = botApi.calls.length;
    const caption = `/cookie${domain ? ` ${domain}` : ''}`;
    botApi.enqueueDocument(42, 42, caption, message, id, { filename: name, size: mode?.advertisedSize, language, ...route });
    if (route.expectedSilent) {
      await sleep(200);
      assert.equal(botApi.calls.slice(beforeCalls).some(call => call.method === 'sendmessage'), false, 'silent-rejection document must not receive a reply');
      const result = { id, reply: { data: { text: '' } }, beforeRows, beforeOps, beforeWatermark, afterRows: await fixtureSql('SELECT * FROM cookies ORDER BY id'), afterOps: await fixtureSql('SELECT count(*) FROM runtime_operations'), afterWatermark: await fixtureSql('SELECT * FROM runtime_personal_cookie_mutation_watermarks WHERE actor_id=42'), newCalls: botApi.downloadCalls.slice(beforeDownloads), beforeDownloads };
      assert.equal(result.newCalls.length, 0, 'silent-rejection document must not fetch Telegram file');
      return result;
    }
    const reply = await waitFor(() => botApi.calls.slice(beforeCalls).find(call => call.method === 'sendmessage' && Number(call.data.chat_id) === 42), 'cookie upload reply');
    assert(!reply.data.text.includes(cookieSecret));
    assert(!reply.data.text.includes(id) && !reply.data.text.includes(name));
    assert(!/https?:\/\//i.test(reply.data.text));
    assert(!JSON.stringify(botApi.calls.slice(beforeCalls)).includes(cookieSecret));
    const newCalls = botApi.downloadCalls.slice(beforeDownloads);
    if (mode?.noFetch || route.expectedSilent) assert.equal(newCalls.length, 0, 'preflight/route rejection must not fetch Telegram file');
    if (mode?.fetch) await waitFor(() => botApi.downloadCalls.slice(beforeDownloads).some(call => call.kind === mode.fetch), 'expected Telegram document fetch');
    await sleep(50);
    return { id, reply, beforeRows, beforeOps, beforeWatermark, afterRows: await fixtureSql('SELECT * FROM cookies ORDER BY id'), afterOps: await fixtureSql('SELECT count(*) FROM runtime_operations'), afterWatermark: await fixtureSql('SELECT * FROM runtime_personal_cookie_mutation_watermarks WHERE actor_id=42'), newCalls, beforeDownloads };
  }
  const assertNoMutation = result => {
    assert.equal(result.afterRows, result.beforeRows, 'failed/rejected document does not mutate cookie rows');
    assert.equal(result.afterOps, result.beforeOps, 'cookie uploads do not admit media jobs');
    assert.equal(result.afterWatermark, result.beforeWatermark, 'failed/rejected document does not advance the message watermark');
  };
  const rows = () => fixtureSql('SELECT id,domain,content,is_valid,is_pool FROM cookies WHERE user_id=42 AND is_pool=0 ORDER BY id');
  const assertReplySafe = text => { assert(!text.includes(cookieSecret)); assert(!/https?:\/\//i.test(text)); };
  let release;
  try {
    await fixtureSql('DELETE FROM cookies WHERE user_id=42 AND is_pool=0');
    const baselineOther = await fixtureSql('SELECT * FROM cookies WHERE user_id<>42 OR is_pool=1 ORDER BY id');
    const good = await upload(Buffer.from(cookie()), { name: 'youtube.com.txt', mode: { fetch: 'file' } });
    assert.match(good.reply.data.text, /youtube\.com/i);
    assert.match(good.reply.data.text, /2/);
    assert.match(good.reply.data.text, /Saved|cookie/i);
    assert.equal(await fixtureSql("SELECT domain FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='youtube.com'"), 'youtube.com');
    assert.equal((await fixtureSql("SELECT count(*) FROM cookies WHERE user_id=42 AND is_pool=0 AND content LIKE '%HttpOnly_auth%'")), '1');
    assert.equal(botApi.downloadCalls.slice(good.beforeDownloads).filter(call => call.kind === 'getFile').length, 1);
    assert.equal(botApi.downloadCalls.slice(good.beforeDownloads).filter(call => call.kind === 'file').length, 1);
    const originalId = Number(await fixtureSql("SELECT id FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='youtube.com'"));
    const mediaCookie = `# Netscape HTTP Cookie File\n127.0.0.1\tFALSE\t/\tFALSE\t4102444800\tfixture_auth\t${cookieSecret}\n`;
    await upload(Buffer.from(mediaCookie), { name: '127.0.0.1.txt', mode: { fetch: 'file' } });
    const mediaCookieId = Number(await fixtureSql("SELECT id FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='127.0.0.1'"));
    await fixtureSql("UPDATE dl_user_settings SET proxy_enabled=0,quality='best' WHERE user_id=42");
    const h = commandHarness(context, 1_100_000);
    const mediaUrl = `http://127.0.0.1:${sourcePort}/cookie-media?case=cookie-upload`;
    const delivered = await h.execute('uploaded personal cookie authenticates media download', `/video --height best --video-codec h264 --container mp4 --send-as-file false ${mediaUrl}`, 'sendvideo',
      { video: true, audio: true, durationMin: 0.8, durationMax: 1.5 }, { filename: 'media.mp4', mimeType: 'video/mp4' });
    assert.equal((await h.request(delivered.operationId)).network.cookie.id, mediaCookieId, 'real media operation must select the uploaded cookie row');
    const originalOperation = await fixtureSql(`SELECT * FROM runtime_operations WHERE id='${delivered.operationId}'`);
    const originalReceipt = await fixtureSql(`SELECT * FROM runtime_delivery_units WHERE operation_id='${delivered.operationId}' ORDER BY attempt,ordinal`);
    const originalCache = await fixtureSql(`SELECT * FROM runtime_media_cache WHERE cache_key=(SELECT cache_key FROM runtime_operations WHERE id='${delivered.operationId}')`);
    sourceFixture.closeGate();
    const sendCount = botApi.sendCalls;
    const blocker = await h.submit(`/video --height best --video-codec h264 --container mp4 --send-as-file false http://127.0.0.1:${sourcePort}/gated?case=cookie-upload-revoke`);
    await waitFor(() => sourceFixture.gatedResponses.size > 0, 'cookie-backed worker reached gated source');
    const queued = await h.submit(`/video --height best --video-codec h264 --container mp4 --send-as-file false ${mediaUrl}&queued=1`);
    assert((await queryJob(queued.operationId)).startsWith('queued|'));
    const frozen = await h.request(queued.operationId);
    assert.equal(frozen.network.cookie.id, mediaCookieId);
    const protectedRequests = sourceFixture.protectedRequests;
    const freshMedia = `# Netscape HTTP Cookie File\n127.0.0.1\tFALSE\t/\tFALSE\t4102440800\tfixture_auth\t${cookieSecret}\n`;
    const freshUpload = await upload(Buffer.from(freshMedia), { name: '127.0.0.1.txt', mode: { fetch: 'file' } });
    assert.match(freshUpload.reply.data.text, /Saved|cookie/i);
    const freshCookieId = Number(await fixtureSql("SELECT id FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='127.0.0.1'"));
    assert.notEqual(freshCookieId, mediaCookieId, 'Telegram replacement creates a fresh media cookie row');
    assert.deepEqual(await h.request(queued.operationId), frozen, 'queued job retains its old cookie reference');
    sourceFixture.releaseGatedResponses();
    await waitFor(async () => (await queryJob(blocker.operationId)).startsWith('failed|') && (await queryJob(queued.operationId)).startsWith('failed|'), 'replacement invalidates active and queued cookie references');
    assert.equal(sourceFixture.protectedRequests, protectedRequests, 'queued replacement failure must not reselect network credentials');
    assert.equal(botApi.sendCalls, sendCount, 'invalidated cookie references must not send media');
    assert.equal(await fixtureSql(`SELECT * FROM runtime_operations WHERE id='${delivered.operationId}'`), originalOperation);
    assert.equal(await fixtureSql(`SELECT * FROM runtime_delivery_units WHERE operation_id='${delivered.operationId}' ORDER BY attempt,ordinal`), originalReceipt);
    assert.equal(await fixtureSql(`SELECT * FROM runtime_media_cache WHERE cache_key=(SELECT cache_key FROM runtime_operations WHERE id='${delivered.operationId}')`), originalCache);
    const freshDelivery = await h.execute('new admission selects the replacement uploaded cookie', `/video --height best --video-codec h264 --container mp4 --send-as-file false ${mediaUrl}&fresh=1`, 'sendvideo',
      { video: true, audio: true, durationMin: 0.8, durationMax: 1.5 }, { filename: 'media.mp4', mimeType: 'video/mp4' });
    assert.equal((await h.request(freshDelivery.operationId)).network.cookie.id, freshCookieId, 'new admission uses the fresh replacement cookie');

    const explicit = await upload(Buffer.from(cookie('replacement')), { name: 'arbitrary.txt', domain: '.youtube.com', mode: { fetch: 'file' } });
    assertReplySafe(explicit.reply.data.text);
    const replacedId = Number(await fixtureSql("SELECT id FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='youtube.com'"));
    assert.notEqual(replacedId, originalId, 'replacement uses a fresh cookie row');
    assert.equal(await fixtureSql("SELECT domain FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='youtube.com'"), 'youtube.com', 'explicit leading-dot domain is canonicalized');
    assert.equal(await fixtureSql("SELECT count(*) FROM cookies WHERE user_id=42 AND is_pool=0 AND domain IN ('youtube.com','.youtube.com')"), '1');
    assert.equal(await fixtureSql('SELECT content FROM cookies WHERE id=' + replacedId), cookie('replacement'));

    const invalidBodies = [Buffer.from('not Netscape'), Buffer.from('# Netscape HTTP Cookie File\nexample.test\tFALSE\t/\tFALSE\t0\tname\n'), Buffer.from([0x23,0x20,0xff,0xfe])];
    for (const bytes of invalidBodies) assertNoMutation(await upload(bytes, { name: 'bad.test.txt', mode: { fetch: 'file' } }));
    assertNoMutation(await upload(Buffer.from(cookie()), { name: 'big.test.txt', mode: { advertisedSize: 524289, noFetch: true } }));
    assertNoMutation(await upload(Buffer.from(cookie()), { name: 'lying.test.txt', mode: { declaredSize: 1, fileMode: 'chunked-oversize', fetch: 'file' } }));
    for (const getFileMode of ['invalid-path', 'redirect', 'failure', 'network-error']) {
      const rejected = await upload(Buffer.from(cookie()), { name: 'badpath.test.txt', mode: { getFileMode, fetch: 'getFile' } });
      assertNoMutation(rejected);
      assert(!rejected.newCalls.some(call => call.kind === 'file'), `${getFileMode} getFile response must not follow/download`);
    }
    const downloadFailure = await upload(Buffer.from(cookie()), { name: 'network.test.txt', mode: { fileMode: 'failure', fetch: 'file' } });
    assertNoMutation(downloadFailure);
    const tooLarge = await upload(Buffer.alloc(524289, 65), { name: 'chunked.test.txt', mode: { advertisedSize: 1, declaredSize: 1, fileMode: 'chunked-oversize', fetch: 'file' } });
    assertNoMutation(tooLarge);
    const delayed = await upload(Buffer.from(cookie()), { name: 'timeout.test.txt', mode: { delayMs: 2500, fetch: 'getFile' } });
    assertNoMutation(delayed);
    const declaredOversize = await upload(Buffer.from(cookie()), { name: 'declared-large.test.txt', mode: { declaredSize: 524289, fetch: 'getFile' } });
    assertNoMutation(declaredOversize);
    assert(!declaredOversize.newCalls.some(call => call.kind === 'file'));
    const redirectedFile = await upload(Buffer.from(cookie()), { name: 'redirect-file.test.txt', mode: { fileMode: 'redirect', fetch: 'file' } });
    assertNoMutation(redirectedFile);
    const recovered = await upload(Buffer.from(cookie()), { name: 'recovery.test.txt', domain: 'recovery.test', mode: { fetch: 'file' } });
    assert.match(recovered.reply.data.text, /Saved|cookie/i, 'valid upload succeeds after transport failures');

    const localFiles = await context.prepareLocalCookieFiles(Buffer.from(cookie()));
    assert.deepEqual(Object.keys(localFiles).sort(), ['directory', 'missing', 'outside', 'parentSymlink', 'oversize', 'symlink', 'valid'].sort());
    const beforeLocal = botApi.downloadCalls.length;
    const localValid = await upload(Buffer.from(cookie()), { name: 'local.test.txt', domain: 'local.test', mode: { localRemotePath: localFiles.valid, fetch: 'getFile' } });
    assert.match(localValid.reply.data.text, /Saved|cookie/i);
    assert.deepEqual(botApi.downloadCalls.slice(beforeLocal).map(call => call.kind), ['getFile']);
    const localValidId = Number(await fixtureSql("SELECT id FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='local.test'"));
    assert(localValidId > 0, 'local Bot API document is stored');
    for (const key of ['oversize', 'symlink', 'parentSymlink', 'outside', 'missing', 'directory']) {
      const beforePathCalls = botApi.downloadCalls.length;
      const failed = await upload(Buffer.from(cookie()), { name: `${key}.test.txt`, domain: `${key}.test`, mode: { localRemotePath: localFiles[key], fetch: 'getFile' } });
      assertNoMutation(failed);
      assert(!botApi.downloadCalls.slice(beforePathCalls).some(call => call.kind === 'file'), `${key} local path must not be downloaded`);
    }

    for (const [actor, chat, caption, options] of [
      [99, 42, '/cookie', { expectedSilent: true }],
      [42, -10042, '/cookie', { chatType: 'group', expectedSilent: true }],
      [42, 42, '/cookie@other_bot', { expectedSilent: true }],
    ]) {
      const id = fileId(); botApi.setDocumentFile(id, Buffer.from(cookie()));
      const before = botApi.downloadCalls.length; const callStart = botApi.calls.length;
      botApi.enqueueDocument(actor, chat, caption, messageId++, id, { filename: 'silent.test.txt', ...options });
      await sleep(150);
      assert.equal(botApi.downloadCalls.length, before);
      assert.equal(botApi.calls.slice(callStart).some(call => call.method === 'sendmessage'), false);
    }
    const noCaptionId = fileId(); botApi.setDocumentFile(noCaptionId, Buffer.from(cookie()));
    const noCaptionBefore = botApi.downloadCalls.length; const noCaptionCalls = botApi.calls.length;
    botApi.enqueueDocument(42, 42, undefined, messageId++, noCaptionId, { filename: 'silent.test.txt' });
    await sleep(150);
    assert.equal(botApi.downloadCalls.length, noCaptionBefore);
    assert.equal(botApi.calls.slice(noCaptionCalls).some(call => call.method === 'sendmessage'), false);
    const botUpload = await upload(Buffer.from(cookie()), { name: 'bot.test.txt', fromBot: 998877, expectedSilent: true });
    assertNoMutation(botUpload);
    const topicUpload = await upload(Buffer.from(cookie()), { name: 'topic.test.txt', topic: 44, expectedSilent: true });
    assertNoMutation(topicUpload);

    const russian = await withRussianPreference(fixtureSql, () => upload(Buffer.from(cookie()), { name: 'ru.test.txt', language: 'en', mode: { fetch: 'file' } }));
    assert.match(russian.reply.data.text, /сохран|cookie/i);
    assertReplySafe(russian.reply.data.text);
    const english = await upload(Buffer.from(cookie()), { name: 'en.test.txt', mode: { fetch: 'file' } });
    assert.match(english.reply.data.text, /Saved|cookie/i);

    const holdId = fileId(); botApi.setDocumentFile(holdId, Buffer.from(cookie('held')));
    release = botApi.holdDocumentFile(holdId);
    const heldBefore = await rows();
    const heldCalls = botApi.calls.length;
    botApi.enqueueDocument(42, 42, '/cookie held.test', messageId++, holdId, { filename: 'held.test.txt' });
    await waitFor(() => botApi.downloadCalls.some(call => call.kind === 'getFile' && call.fileId === holdId), 'held upload preflight');
    await fixtureSql("UPDATE users SET ban_until=unixepoch('now')*1000000+600000000 WHERE id=42");
    release(); release = undefined;
    await waitFor(() => botApi.calls.slice(heldCalls).some(call => call.method === 'sendmessage'), 'banned held upload reply');
    assert.equal(await rows(), heldBefore, 'ban recheck after download prevents insert');
    await fixtureSql(`UPDATE users SET ban_until=${literal(savedUser.ban_until)} WHERE id=42`);

    const replayId = messageId++;
    const firstReplay = await upload(Buffer.from(cookie('replay-a')), { name: 'fence.test.txt', domain: 'fence.test', message: replayId, mode: { fetch: 'file' } });
    const firstRowId = Number(await fixtureSql("SELECT id FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='fence.test'"));
    const removeStart = botApi.calls.length;
    const deleteMessageId = messageId++;
    botApi.enqueue(42, 42, '/cookie del fence.test', deleteMessageId, 'en');
    await waitFor(() => botApi.calls.slice(removeStart).some(call => call.method === 'sendmessage'), 'delete upload fixture cookie');
    await restartApp();
    const replacement = await upload(Buffer.from(cookie('replay-b')), { name: 'fence.test.txt', domain: 'fence.test', message: messageId++, mode: { fetch: 'file' } });
    const secondRowId = Number(await fixtureSql("SELECT id FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='fence.test'"));
    assert.notEqual(secondRowId, firstRowId);
    const replayStart = botApi.calls.length;
    botApi.enqueueDocument(42, 42, '/cookie fence.test', replayId, firstReplay.id, { filename: 'fence.test.txt' });
    await waitFor(() => botApi.calls.slice(replayStart).some(call => call.method === 'sendmessage'), 'stale upload replay acknowledgement');
    assert.equal(await fixtureSql("SELECT id FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='fence.test'"), String(secondRowId));
    const staleDeleteStart = botApi.calls.length;
    botApi.enqueue(42, 42, '/cookie del fence.test', deleteMessageId, 'en');
    await waitFor(() => botApi.calls.slice(staleDeleteStart).some(call => call.method === 'sendmessage'), 'stale delete replay acknowledgement');
    assert.equal(await fixtureSql("SELECT id FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='fence.test'"), String(secondRowId), 'old delete replay cannot remove post-restart replacement');
    const deniedCount = botApi.downloadCalls.length;
    await fixtureSql("UPDATE users SET ban_until=unixepoch('now')*1000000+600000000 WHERE id=42");
    const deniedId = fileId(); botApi.setDocumentFile(deniedId, Buffer.from(cookie()));
    botApi.enqueueDocument(42, 42, '/cookie denied.test', messageId++, deniedId, { filename: 'denied.test.txt' });
    await sleep(250);
    assert.equal(botApi.downloadCalls.length, deniedCount, 'preflight ban avoids getFile and file requests');
    await fixtureSql(`UPDATE users SET ban_until=${literal(savedUser.ban_until)} WHERE id=42`);

    assert.equal(await fixtureSql('SELECT * FROM cookies WHERE user_id<>42 OR is_pool=1 ORDER BY id'), baselineOther);
    assert.equal(Number(await fixtureSql('SELECT count(*) FROM runtime_operations')), Number(initialOperations) + 4,
      'the successful initial, blocker, queued, and fresh-cookie media scenarios are the only admitted operations');
    assert(!JSON.stringify(botApi.calls.slice(startCalls)).includes(cookieSecret));
    assert.equal(botApi.error, undefined);
    await assertWorkspaceClean();
    return 'personal cookie document acceptance: domain inference/override, validated bounded download, replacement, routing/authorization, EN/RU, replay fencing, and safe replies passed';
  } finally {
    release?.();
    sourceFixture.releaseGatedResponses();
    await fixtureSql(`UPDATE users SET role=${literal(savedUser.role)},ban_until=${literal(savedUser.ban_until)} WHERE id=42`);
    await fixtureSql('DELETE FROM cookies WHERE user_id=42 AND is_pool=0');
    for (const row of savedPersonal) await fixtureSql(`INSERT INTO cookies(${columns.join(',')}) VALUES(${columns.map(name => literal(row[name])).join(',')})`);
    if (savedSettingsObject) await fixtureSql(`UPDATE dl_user_settings SET ${Object.entries(savedSettingsObject).map(([name, value]) => `${name}=${literal(value)}`).join(',')} WHERE user_id=42`);
    else await fixtureSql('DELETE FROM dl_user_settings WHERE user_id=42');
    await fixtureSql('DELETE FROM runtime_personal_cookie_mutation_watermarks WHERE actor_id=42');
    if (savedWatermark) await fixtureSql(`INSERT INTO runtime_personal_cookie_mutation_watermarks(actor_id,message_id) VALUES(${literal(savedWatermark.split('|')[0])},${literal(savedWatermark.split('|')[1])})`);
    assert.equal(await fixtureSql('SELECT * FROM cookies ORDER BY id'), savedCookies, 'restore all cookie rows exactly');
    assert.equal(await fixtureSql('SELECT * FROM dl_user_settings WHERE user_id=42'), savedSettings, 'restore user settings exactly');
  }
}
