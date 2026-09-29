import assert from 'node:assert/strict';
import { setTimeout as sleep } from 'node:timers/promises';
import { commandHarness } from './command-harness.mjs';

const cookie = value => Buffer.from(`# Netscape HTTP Cookie File\n.example.test\tTRUE\t/\tTRUE\t4102444800\tsessionid\t${value}\n`);

export async function runCookieUrlScenarios(context) {
  const { botApi, fixtureSql, waitFor, restartApp, sourceFixture, sourcePort, cookieSecret, queryJob, assertWorkspaceClean } = context;
  const quote = value => value === null ? 'NULL' : typeof value === 'number' ? String(value) : `'${String(value).replaceAll("'", "''")}'`;
  const allCookies = await fixtureSql('SELECT * FROM cookies ORDER BY id');
  const cookieColumns = ['id', 'user_id', 'domain', 'content', 'is_valid', 'created_at', 'updated_at', 'validated_at', 'is_pool', 'label', 'avatar_url', 'content_hash', 'session_key'];
  const personal = JSON.parse(await fixtureSql(`SELECT json_group_array(json_object(${cookieColumns.flatMap(name => [quote(name), name]).join(',')})) FROM (SELECT * FROM cookies WHERE user_id=42 AND is_pool=0 ORDER BY id)`));
  const userColumns = (await fixtureSql("SELECT name FROM pragma_table_info('users') ORDER BY cid")).trim().split('\n');
  const user = await fixtureSql(`SELECT json_object(${userColumns.flatMap(name => [quote(name), name]).join(',')}) FROM users WHERE id=42`);
  const settings = await fixtureSql('SELECT * FROM dl_user_settings WHERE user_id=42');
  const settingsColumns = (await fixtureSql("SELECT name FROM pragma_table_info('dl_user_settings') ORDER BY cid")).trim().split('\n');
  const settingsRow = await fixtureSql(`SELECT json_object(${settingsColumns.flatMap(name => [quote(name), name]).join(',')}) FROM dl_user_settings WHERE user_id=42`);
  const allSettings = await fixtureSql('SELECT * FROM dl_user_settings ORDER BY user_id');
  let expectedSettings = allSettings;
  const watermark = await fixtureSql('SELECT * FROM runtime_personal_cookie_mutation_watermarks WHERE actor_id=42');
  const others = await fixtureSql('SELECT * FROM cookies WHERE user_id<>42 OR is_pool=1 ORDER BY id');
  let operations = await fixtureSql('SELECT * FROM runtime_operations ORDER BY id');
  let deliveryUnits = await fixtureSql('SELECT * FROM runtime_delivery_units ORDER BY operation_id,attempt,ordinal');
  let mediaCache = await fixtureSql('SELECT * FROM runtime_media_cache ORDER BY cache_key');
  const callsStart = botApi.calls.length;
  let messageId = 1_800_000;
  let index = 0;
  const path = name => `/cookie-import/${name}`;
  const origin = `http://127.0.0.1:${sourcePort}`;
  const rows = () => fixtureSql('SELECT id,domain,content,is_valid,is_pool FROM cookies WHERE user_id=42 AND is_pool=0 ORDER BY id');
  const mutationState = async () => ({ rows: await rows(), watermark: await fixtureSql('SELECT * FROM runtime_personal_cookie_mutation_watermarks WHERE actor_id=42') });
  async function send(url, domain, options = {}) {
    const id = messageId++;
    const sourcePath = options.sourcePath ?? `valid-${++index}`;
    const data = options.body ?? cookie(`token-${id}`);
    if (!options.noRoute && !options.preserveRoute) sourceFixture.setCookieImportResponse(new URL(url).pathname, options.response ?? { body: data, headers: { 'content-type': options.contentType ?? 'text/plain' } });
    const before = await mutationState();
    const requestStart = sourceFixture.cookieImportRequests.length;
    const callStart = botApi.calls.length;
    const downloadsBefore = botApi.downloadCalls?.length ?? 0;
    const command = options.command ?? `/cookie ${url}${domain ? ` ${domain}` : ''}`;
    botApi.enqueue(options.actor ?? 42, options.chat ?? 42, command, id, options.language ?? 'en', options.route ?? {});
    let reply;
    if (options.silent) {
      await sleep(180);
      assert.equal(botApi.calls.slice(callStart).some(c => c.method === 'sendmessage'), false, 'unauthorized command must be silent');
    } else {
      reply = await waitFor(() => botApi.calls.slice(callStart).find(c => c.method === 'sendmessage' && Number(c.data.chat_id) === (options.chat ?? 42)), `cookie URL reply ${sourcePath}`);
      assert(!reply.data.text.includes(cookieSecret));
      assert(!reply.data.text.includes(new URL(url).searchParams.get('token') ?? '\0'));
      assert(!/https?:\/\//i.test(reply.data.text), 'reply must not disclose source URL');
    }
    const requests = sourceFixture.cookieImportRequests.slice(requestStart);
    if (options.fetch === false || options.silent) assert.equal(requests.length, 0, 'rejected URL must not fetch source');
    else assert.equal(requests.length, options.fetch ?? 1, 'source request count');
    assert.equal(botApi.downloadCalls?.length ?? 0, downloadsBefore, 'URL import must not invoke Telegram file download');
    await sleep(30);
    return { before, after: await mutationState(), reply, requests, id };
  }
  function assertNoMutation(result) { assert.deepEqual(result.after, result.before, 'failed import leaves personal rows and watermark unchanged'); }
  try {
    await fixtureSql("UPDATE users SET language='en' WHERE id=42");
    await fixtureSql('DELETE FROM cookies WHERE user_id=42 AND is_pool=0');
    sourceFixture.clearCookieImportResponses();
    const original = await send(`${origin}${path('original-host')}?token=private-query`);
    assert.match(original.reply.data.text, /Personal cookies were saved\./);
    assert.equal(await fixtureSql("SELECT domain FROM cookies WHERE user_id=42 AND is_pool=0"), '127.0.0.1');
    assert.equal(original.requests[0].path, `${path('original-host')}?token=private-query`);
    assert.equal(original.requests[0].headers.cookie, undefined);
    assert.equal(original.requests[0].headers.authorization, undefined);
    assert.equal(original.requests[0].headers['proxy-authorization'], undefined);

    const canonical = await send(`${origin}${path('canonical')}`, '.Example.COM.');
    assert.match(canonical.reply.data.text, /Personal cookies were saved\./);
    assert.equal(await fixtureSql("SELECT domain FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='example.com'"), 'example.com');
    const firstId = await fixtureSql("SELECT id FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='example.com'");
    const canonicalReplacement = await send(`${origin}${path('canonical-replacement')}`, 'example.com', { sourcePath: 'canonical-replacement' });
    assert.match(canonicalReplacement.reply.data.text, /Personal cookies were saved\./);
    assert.notEqual(await fixtureSql("SELECT id FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='example.com'"), firstId, 'replacement allocates a fresh cookie row');
    await fixtureSql("UPDATE users SET language='ru' WHERE id=42");
    const russian = await send(`${origin}${path('russian')}`, undefined, { language: 'en' });
    assert.match(russian.reply.data.text, /\p{Script=Cyrillic}/u, 'saved Russian preference takes precedence over Telegram metadata');
    await fixtureSql("UPDATE users SET language='en' WHERE id=42");
    sourceFixture.setCookieImportResponse(path('redirect-start'), { redirect: path('redirect-done') });
    sourceFixture.setCookieImportResponse(path('redirect-done'), { body: cookie('redirect'), headers: { 'content-type': 'text/plain' } });
    const redirect = await send(`${origin}${path('redirect-start')}`, 'redirect.example', { preserveRoute: true, fetch: 2 });
    assert.match(redirect.reply.data.text, /Personal cookies were saved\./);
    assert.deepEqual(redirect.requests.map(request => new URL(request.path, origin).pathname), [path('redirect-start'), path('redirect-done')]);
    assert(redirect.requests.every(request => !request.headers.cookie && !request.headers.authorization && !request.headers['proxy-authorization']));

    const maxPrefix = cookie('max');
    const maxBody = Buffer.concat([maxPrefix, Buffer.from('#'), Buffer.alloc(524288 - maxPrefix.length - 1, 65)]);
    const max = await send(`${origin}${path('exact-limit')}`, 'limit.example', { body: maxBody, sourcePath: 'exact-limit' });
    assert.match(max.reply.data.text, /Personal cookies were saved\./);
    const invalidBodies = [Buffer.from('bad'), Buffer.from([0x23, 0x20, 0xff, 0xfe]), Buffer.from('# Netscape HTTP Cookie File\nexample.test\tFALSE\t/\tFALSE\t0\tname\n')];
    for (const [n, body] of invalidBodies.entries()) assertNoMutation(await send(`${origin}${path(`invalid-${n}`)}`, undefined, { body, sourcePath: `invalid-${n}` }));
    for (const [name, response] of [
      ['oversize', { body: Buffer.alloc(524289, 32), chunked: true }],
      ['chunked-invalid', { body: Buffer.alloc(524289, 32), chunked: true }],
      ['misleading-length', { body: Buffer.alloc(524289, 32), contentLength: true, headers: { 'content-length': '1' } }],
      ['non2xx', { status: 503, body: cookie('no') }],
      ['timeout', { body: cookie('late'), delayMs: 2600 }],
      ['encoded', { body: cookie('encoded'), headers: { 'content-type': 'text/plain', 'content-encoding': 'gzip' } }],
    ]) {
      const failed = await send(`${origin}${path(name)}`, undefined, { response });
      assertNoMutation(failed);
      assert.doesNotMatch(failed.reply.data.text, /Saved|cookie added|imported/i, `${name} must report failure, not apparent success`);
      if (['non2xx', 'timeout', 'encoded'].includes(name)) assert.match(failed.reply.data.text, /cookie URL could not be fetched/i, `${name} must return the fetch-error reply`);
    }

    for (const [name, url] of [
      ['wrong-port', `http://127.0.0.1:${sourcePort + 1}/nope`],
      ['other-loopback', `http://127.0.0.2:${sourcePort}/nope`],
      ['metadata', 'http://169.254.169.254/latest/meta-data/'],
      ['link-local', 'http://169.254.1.1/'],
      ['credentials', `http://user:pass@127.0.0.1:${sourcePort}${path('x')}`],
      ['fragment', `${origin}${path('x')}#fragment`],
      ['unsupported', 'file:///etc/passwd'],
      ['control', `${origin}${path('x')}%0d%0a`],
    ]) assertNoMutation(await send(url, undefined, { noRoute: true, fetch: false }));

    sourceFixture.setCookieImportResponse(path('redirect-private'), { redirect: 'http://169.254.169.254/latest/meta-data/' });
    assertNoMutation(await send(`${origin}${path('redirect-private')}`, undefined, { preserveRoute: true, fetch: 1 }));
    sourceFixture.setCookieImportResponse(path('redirect-loop'), { redirect: path('redirect-loop') });
    assertNoMutation(await send(`${origin}${path('redirect-loop')}`, undefined, { preserveRoute: true, fetch: 6 }));
    for (const route of [
      { actor: 99, chat: 99, silent: true }, { actor: 42, chat: -1800000, silent: true, route: { chatType: 'group' } },
      { silent: true, route: { isBot: true } }, { silent: true, route: { senderChat: { id: -1001, title: 'Fixture channel', type: 'channel' } } }, { silent: true, route: { thread: 7 } },
    ]) assertNoMutation(await send(`${origin}${path('denied')}`, undefined, { sourcePath: 'denied', ...route, fetch: false }));
    const viaBotPrivate = await send(`${origin}${path('via-bot-private')}`, undefined, { route: { viaBot: 123456 } });
    assert.match(viaBotPrivate.reply.data.text, /Personal cookies were saved\./, 'private via_bot alone remains eligible under the current incoming-route rules');
    assertNoMutation(await send(`${origin}${path('foreign')}`, undefined, { command: `/cookie@other_bot ${origin}${path('foreign')}`, silent: true, fetch: false }));
    await fixtureSql("UPDATE users SET ban_until=unixepoch('now')*1000000+600000000 WHERE id=42");
    assertNoMutation(await send(`${origin}${path('banned')}`, undefined, { sourcePath: 'banned', fetch: false }));
    await fixtureSql(`UPDATE users SET ban_until=${quote(JSON.parse(user).ban_until)} WHERE id=42`);
    sourceFixture.setCookieImportResponse(path('ban-during-fetch'), { body: cookie('late-ban'), delayMs: 350 });
    const delayedId = messageId++;
    const delayedCalls = botApi.calls.length;
    const banFetchStart = sourceFixture.cookieImportRequests.length;
    const delayedBefore = await mutationState();
    botApi.enqueue(42, 42, `/cookie ${origin}${path('ban-during-fetch')}`, delayedId, 'en');
    await waitFor(() => sourceFixture.cookieImportRequests.some(request => request.path === path('ban-during-fetch')), 'delayed URL import request');
    await fixtureSql("UPDATE users SET ban_until=unixepoch('now')*1000000+600000000 WHERE id=42");
    await waitFor(() => botApi.calls.slice(delayedCalls).some(call => call.method === 'sendmessage'), 'post-fetch ban recheck reply');
    assert.equal(sourceFixture.cookieImportRequests.length - banFetchStart, 1, 'ban-during-fetch source request count');
    assert.deepEqual(await mutationState(), delayedBefore, 'ban applied during fetch prevents cookie mutation');
    await fixtureSql(`UPDATE users SET ban_until=${quote(JSON.parse(user).ban_until)} WHERE id=42`);
    const overtakenResponse = { body: cookie('overtaken'), delayMs: 350 };
    const overtakenBefore = await mutationState();
    const overtakenId = messageId++;
    const overtakenRequests = sourceFixture.cookieImportRequests.length;
    const overtakenCalls = botApi.calls.length;
    sourceFixture.setCookieImportResponse(path('overtaken'), overtakenResponse);
    botApi.enqueue(42, 42, `/cookie ${origin}${path('overtaken')} newer.example`, overtakenId, 'en');
    await waitFor(() => sourceFixture.cookieImportRequests.some(request => request.path === path('overtaken')), 'overtaken URL fetch begins');
    await fixtureSql(`DELETE FROM cookies WHERE user_id=42 AND is_pool=0`);
    await fixtureSql(`INSERT INTO cookies(user_id,domain,content,is_valid,is_pool,created_at,updated_at) VALUES(42,'newer.example',${quote(cookie('newer').toString())},1,0,unixepoch('now')*1000000,unixepoch('now')*1000000)`);
    await fixtureSql(`INSERT INTO runtime_personal_cookie_mutation_watermarks(actor_id,message_id) VALUES(42,${overtakenId + 1000}) ON CONFLICT(actor_id) DO UPDATE SET message_id=excluded.message_id`);
    const overtakenCommitted = await mutationState();
    await waitFor(() => botApi.calls.slice(overtakenCalls).some(call => call.method === 'sendmessage'), 'overtaken fetch acknowledgement');
    assert.equal(sourceFixture.cookieImportRequests.length - overtakenRequests, 1, 'overtaken import fetches source exactly once');
    assert.deepEqual(await mutationState(), overtakenCommitted, 'delayed import cannot overwrite a newer cookie commit or watermark');
    assert.notDeepEqual(overtakenCommitted, overtakenBefore);
    messageId = overtakenId + 1001;

    const mediaCookieBody = Buffer.from(`# Netscape HTTP Cookie File\n127.0.0.1\tFALSE\t/\tFALSE\t4102440800\tfixture_auth\t${cookieSecret}\n`);
    sourceFixture.setCookieImportResponse(path('media-auth'), { body: mediaCookieBody, headers: { 'content-type': 'text/plain' } });
    const mediaAuthImport = await send(`${origin}${path('media-auth')}`, undefined, { preserveRoute: true });
    assert.match(mediaAuthImport.reply.data.text, /Personal cookies were saved\./);
    assert.equal(await fixtureSql('SELECT * FROM runtime_operations ORDER BY id'), operations, 'cookie URL import alone does not admit media work');
    assert.equal(await fixtureSql('SELECT * FROM runtime_media_cache ORDER BY cache_key'), mediaCache, 'cookie URL import alone does not change media cache');
    const mediaCookieId = await fixtureSql("SELECT id FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='127.0.0.1'");
    assert.notEqual(mediaCookieId, '');
    assert.equal(await fixtureSql('SELECT * FROM dl_user_settings ORDER BY user_id'), allSettings, 'URL imports preserve settings');
    await fixtureSql("UPDATE dl_user_settings SET proxy_enabled=0,quality='best',send_as_file=0 WHERE user_id=42");
    expectedSettings = await fixtureSql('SELECT * FROM dl_user_settings ORDER BY user_id');
    const mediaHarness = commandHarness(context, 1_900_000);
    const authenticatedUrl = `http://127.0.0.1:${sourcePort}/cookie-media?case=cookie-url-auth`;
    const protectedBefore = sourceFixture.protectedRequests;
    const mediaDelivery = await mediaHarness.execute('URL-imported cookie authenticates a real media worker', `/video --height best --video-codec h264 --container mp4 --send-as-file false ${authenticatedUrl}`, 'sendvideo',
      { video: true, audio: true, durationMin: 0.8, durationMax: 1.5 }, { filename: 'media.mp4', mimeType: 'video/mp4' });
    assert.equal(String((await mediaHarness.request(mediaDelivery.operationId)).network.cookie.id), mediaCookieId, 'worker selects the imported cookie row');
    assert(sourceFixture.protectedRequests > protectedBefore, 'real worker probes/downloads the protected source using the imported cookie');
    assert.equal(sourceFixture.error, undefined, 'every protected request carries only the selected cookie');
    operations = await fixtureSql('SELECT * FROM runtime_operations ORDER BY id');
    deliveryUnits = await fixtureSql('SELECT * FROM runtime_delivery_units ORDER BY operation_id,attempt,ordinal');
    mediaCache = await fixtureSql('SELECT * FROM runtime_media_cache ORDER BY cache_key');

    const stale = await send(`${origin}${path('replay')}`, 'replay.example', { sourcePath: 'replay' });
    const replayId = stale.id;
    const replayRow = await fixtureSql("SELECT id FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='replay.example'");
    async function mutate(command) {
      const start = botApi.calls.length;
      botApi.enqueue(42, 42, command, messageId++, 'en');
      await waitFor(() => botApi.calls.slice(start).some(call => call.method === 'sendmessage' && Number(call.data.chat_id) === 42), `cookie mutation ${command}`);
    }
    await mutate('/cookie del replay.example');
    assert.equal(await fixtureSql("SELECT count(*) FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='replay.example'"), '0');
    await mutate('/cookie clear');
    const documentId = 'cookie-url-shared-watermark';
    botApi.setDocumentFile(documentId, cookie('document'));
    const documentCalls = botApi.calls.length;
    botApi.enqueueDocument(42, 42, '/cookie document.example', messageId++, documentId, { filename: 'document.example.txt' });
    await waitFor(() => botApi.calls.slice(documentCalls).some(call => call.method === 'sendmessage' && Number(call.data.chat_id) === 42), 'document mutation after URL import');
    const documentRow = await fixtureSql("SELECT id FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='document.example'");
    await restartApp();
    const beforeReplayRequests = sourceFixture.cookieImportRequests.length;
    const replayCalls = botApi.calls.length;
    botApi.enqueue(42, 42, `/cookie ${origin}${path('replay')} replay.example`, replayId, 'en');
    await waitFor(() => botApi.calls.slice(replayCalls).some(c => c.method === 'sendmessage'), 'replayed cookie URL acknowledgement');
    assert.equal(sourceFixture.cookieImportRequests.length, beforeReplayRequests, 'replay after restart must not fetch');
    assert.equal(await fixtureSql("SELECT count(*) FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='replay.example'"), '0', 'stale URL import cannot undo delete/clear');
    assert.equal(await fixtureSql("SELECT id FROM cookies WHERE user_id=42 AND is_pool=0 AND domain='document.example'"), documentRow, 'URL replay cannot undo newer document upload');
    assert.notEqual(replayRow, '');
    assert.equal(await fixtureSql('SELECT * FROM cookies WHERE user_id<>42 OR is_pool=1 ORDER BY id'), others);
    assert(!JSON.stringify(botApi.calls.slice(callsStart)).includes(cookieSecret));
    assert(!JSON.stringify(botApi.calls.slice(callsStart)).includes('private-query'));
    assert.equal(await fixtureSql('SELECT * FROM runtime_operations ORDER BY id'), operations, 'URL imports do not admit media jobs');
    assert.equal(await fixtureSql('SELECT * FROM runtime_delivery_units ORDER BY operation_id,attempt,ordinal'), deliveryUnits);
    assert.equal(await fixtureSql('SELECT * FROM runtime_media_cache ORDER BY cache_key'), mediaCache);
    assert.equal(await fixtureSql('SELECT * FROM dl_user_settings ORDER BY user_id'), expectedSettings);
    await assertWorkspaceClean();
    return 'native Bot API URL-cookie acceptance covered source safety, bounded validation, redirects, authorization, replacement domain behavior, and restart replay';
  } finally {
    sourceFixture.clearCookieImportResponses();
    const savedUser = JSON.parse(user);
    await fixtureSql(`DELETE FROM cookies WHERE user_id=42 AND is_pool=0`);
    for (const row of personal) await fixtureSql(`INSERT INTO cookies(${cookieColumns.join(',')}) VALUES(${cookieColumns.map(name => quote(row[name])).join(',')})`);
    await fixtureSql(`UPDATE users SET ${userColumns.filter(name => name !== 'id').map(name => `${name}=${quote(savedUser[name])}`).join(',')} WHERE id=42`);
    if (settingsRow) {
      const savedSettings = JSON.parse(settingsRow);
      await fixtureSql(`UPDATE dl_user_settings SET ${settingsColumns.filter(name => name !== 'user_id').map(name => `${name}=${quote(savedSettings[name])}`).join(',')} WHERE user_id=42`);
    }
    await fixtureSql(`DELETE FROM runtime_personal_cookie_mutation_watermarks WHERE actor_id=42`);
    if (watermark) {
      const [actor, id] = watermark.split('|');
      await fixtureSql(`INSERT INTO runtime_personal_cookie_mutation_watermarks(actor_id,message_id) VALUES(${quote(actor)},${quote(id)})`);
    }
    assert.equal(await fixtureSql('SELECT * FROM cookies WHERE user_id<>42 OR is_pool=1 ORDER BY id'), others);
    assert.equal(await fixtureSql('SELECT * FROM cookies ORDER BY id'), allCookies, 'restore exact cookie snapshot');
    assert.equal(await fixtureSql('SELECT * FROM dl_user_settings WHERE user_id=42'), settings);
    assert.equal(await fixtureSql('SELECT * FROM dl_user_settings ORDER BY user_id'), allSettings);
  }
}
