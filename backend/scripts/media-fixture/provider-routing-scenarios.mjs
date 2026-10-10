import assert from 'node:assert/strict';
import { commandHarness } from './command-harness.mjs';
import { galleryImage } from './gallery-source.mjs';

export async function runProviderRoutingScenarios(context) {
  const { botApi, fixtureSql, restartGallery, restartApp, restartGroups, gallerySource, proxyUrl,
    queryJob, waitFor, mediaBytes, primaryBytes, providerSource, cookieSecret } = context;
  const h = commandHarness(context, 2_900_000);
  const actor = 8000000000011, group = -8000000000011;
  const privateRoute = { actor, chat: actor }, groupRoute = { actor, chat: group, thread: 18 };
  const fields = ['proxy_enabled', 'proxy_url', 'send_as_file', 'gallery_zip', 'use_pool_cookies', 'nsfw_blur', 'quality', 'codec', 'container', 'audio_codec'];
  const literal = value => value === null ? 'NULL' : typeof value === 'number' ? String(value) : `'${String(value).replaceAll("'", "''")}'`;
  const previous = new Map();
  const oldRole = await fixtureSql(`SELECT role FROM users WHERE id=${actor}`);
  const oldGroupPolicy = await fixtureSql(`SELECT nsfw_allowed FROM groups WHERE id=${group}`);
  const oldCounters = JSON.parse(await fixtureSql(`SELECT json_group_array(json_object('window',window,'count',count,'reset_at',reset_at)) FROM rate_limits WHERE user_id=${actor}`));
  const cookieIds = [], ruleIds = [];
  const url = code => `https://www.instagram.com/p/${code}/`;
  const command = code => `/video ${url(code)}`;
  const mediaCalls = start => botApi.calls.slice(start).filter(call => ['sendvideo', 'sendphoto', 'senddocument', 'sendmediagroup'].includes(call.method));
  const cacheRows = id => fixtureSql(`SELECT count(*) FROM runtime_media_cache WHERE cache_key=(SELECT cache_key FROM runtime_operations WHERE id='${id}')`);
  const metadata = async id => JSON.parse(await fixtureSql(`SELECT media FROM runtime_media_cache WHERE cache_key=(SELECT cache_key FROM runtime_operations WHERE id='${id}')`));
  const counters = () => fixtureSql(`SELECT window||'|'||count||'|'||reset_at FROM rate_limits WHERE user_id=${actor} ORDER BY window`);
  const expected = [
    { kind: 'photo', filename: 'image.png', mimeType: 'image/png', bytes: galleryImage(1) },
    { kind: 'video', filename: 'media.mp4', mimeType: 'video/mp4', bytes: mediaBytes },
  ];
  function album() { botApi.setExpectedUpload(null); botApi.setExpectedAlbum(expected); }
  async function insertCookie(owner, pool = false) {
    const content = `# Netscape HTTP Cookie File\n.instagram.com\tTRUE\t/\tTRUE\t0\tsessionid\t${cookieSecret}\n`;
    const id = Number(await fixtureSql(`INSERT INTO cookies(user_id,domain,content,is_valid,is_pool,created_at,updated_at) VALUES(${owner},'instagram.com',${literal(content)},1,${Number(pool)},unixepoch('now')*1000000,unixepoch('now')*1000000) RETURNING id`));
    assert(Number.isSafeInteger(id) && id > 0);
    cookieIds.push(id);
    return id;
  }
  async function assertNoPublish(id, sends) {
    assert.equal(botApi.sendCalls, sends, 'unsuccessful source/policy must not issue any media send');
    assert.equal(await fixtureSql(`SELECT count(*) FROM runtime_delivery_units WHERE operation_id='${id}'`), '0');
    assert.equal(await cacheRows(id), '0', 'unsuccessful attempt must not publish cache');
    await context.assertWorkspaceClean();
  }
  async function failed(code, options) {
    album();
    const sends = botApi.sendCalls;
    const sent = await h.submit(command(code), options);
    await waitFor(async () => (await queryJob(sent.operationId)).startsWith('failed|'), `${code} terminal failure`, 45000);
    await assertNoPublish(sent.operationId, sends);
    return sent;
  }
  async function mixed(code, options, spoilers = false) {
    album();
    const sent = await h.submit(command(code), options);
    await h.succeeded(sent.operationId);
    assert.equal((await h.request(sent.operationId)).params.engine, 'auto_fallback_v1');
    const calls = mediaCalls(sent.callStart);
    if (!calls[0]?.cached) assert.deepEqual(botApi.lastGroupBytes, expected.map(x => x.bytes));
    assert.equal(calls.length, 1);
    assert.equal(calls[0].method, 'sendmediagroup');
    assert.deepEqual(calls[0].has_spoilers, [spoilers, spoilers]);
    assert.equal(await fixtureSql(`SELECT planned_artifact_count||'|'||delivered_artifact_count FROM runtime_operations WHERE id='${sent.operationId}'`), '2|2');
    assert.equal(await fixtureSql(`SELECT ordinal||'|'||artifact_count||'|'||status FROM runtime_delivery_units WHERE operation_id='${sent.operationId}' ORDER BY ordinal`), '0|2|sent');
    await context.assertWorkspaceClean();
    return sent;
  }
  async function blocker() {
    const release = providerSource.hold('PRBlocker', 'primary-media');
    const hits = providerSource.events('PRBlocker', 'primary-media').length;
    const sent = await h.submit(command('PRBlocker'));
    await waitFor(() => providerSource.events('PRBlocker', 'primary-media').length > hits, 'held primary occupies native worker');
    return { sent, release };
  }
  try {
    for (const id of [42, actor]) {
      previous.set(id, JSON.parse(await fixtureSql(`SELECT json_object(${fields.flatMap(f => [literal(f), f]).join(',')}) FROM dl_user_settings WHERE user_id=${id}`) || 'null'));
      await fixtureSql(`INSERT INTO dl_user_settings(user_id,created_at,updated_at) VALUES(${id},unixepoch('now')*1000000,unixepoch('now')*1000000) ON CONFLICT(user_id) DO NOTHING`);
      await fixtureSql(`UPDATE dl_user_settings SET proxy_enabled=1,proxy_url=${literal(proxyUrl)},quality='best',codec='avc1',container='mp4',audio_codec='best',send_as_file=0,gallery_zip=0,use_pool_cookies=0,nsfw_blur=1 WHERE user_id=${id}`);
      // This source is disposable. Never silently compete with an imported credential reference.
      assert.equal(await fixtureSql(`SELECT count(*) FROM cookies WHERE user_id=${id} AND domain='instagram.com' AND is_pool=0`), '0');
    }
    await insertCookie(42);
    let actorCookie = await insertCookie(actor);
    const keyword = Number(await fixtureSql("INSERT INTO nsfw_keywords(keyword,note,created_at) VALUES('sensitivecontent','provider routing acceptance',unixepoch('now')*1000000) RETURNING id"));
    ruleIds.push(keyword);
    await restartGallery(true);

    // Native table admission while one real source holds concurrency. Cancel each queued request
    // before any extractor can contact an unimplemented domain. No live source is contacted.
    const held = await blocker();
    try {
      const galleryHosts = ['2ch.su', '4chan.org', 'e-hentai.org', 'e621.net', 'gelbooru.com', 'kemono.cr', 'kemono.party', 'coomer.party', 'nhentai.net', 'civitai.com', 'wallhaven.cc'];
      const table = [
        ...galleryHosts.map(host => [`https://${host}/post`, 'auto_gallery_v1']),
        ...galleryHosts.map(host => [`https://sub.${host}/post`, 'auto_gallery_v1']),
        ...galleryHosts.map(host => [`https://${host}.evil.test/?url=https://${host}/`, 'auto_yt_dlp_v1']),
        ['https://m.vk.com/wall-123_456?x=1', 'auto_gallery_v1'],
        ['https://vk.com/album-123_456', 'auto_gallery_v1'],
        ['https://vk.com/?next=vk.com/wall-123', 'auto_yt_dlp_v1'],
        ['https://evil.test/vk.com/album-123', 'auto_yt_dlp_v1'],
        ['https://vk.com.evil.test/wall-123', 'auto_yt_dlp_v1'],
        ['https://vk.com/path/wall-123', 'auto_yt_dlp_v1'],
        ['https://instagram.com.evil.test/p/PRMeta', 'auto_yt_dlp_v1'],
        ['https://notinstagram.com/p/PRMeta', 'auto_yt_dlp_v1'],
        ['https://www.instagram.com/p/PRMeta', 'auto_fallback_v1'],
        ['https://WWW.INSTAGRAM.COM/p/PRMeta', 'auto_fallback_v1'],
        ['https://4chan.org./post', 'auto_gallery_v1'],
        ['https://youtube.com/watch?v=x&next=vk.com/wall-123', 'auto_yt_dlp_v1'],
        ['https://youtu.be/PRMeta', 'auto_yt_dlp_v1'],
      ];
      const fetched = gallerySource.requests;
      for (const [source, engine] of table) {
        const queued = await h.submit(`/video ${source}`);
        assert((await queryJob(queued.operationId)).startsWith('queued|'));
        assert.equal((await h.request(queued.operationId)).params.engine, engine, `native routing table ${source}`);
        await h.cancel(queued.operationId);
      }
      const image = await h.submit('/image https://youtube.com/watch?v=image');
      assert.equal((await h.request(image.operationId)).params.engine, 'gallery');
      await h.cancel(image.operationId);
      assert.equal(gallerySource.requests, fetched, 'table admissions cannot start any additional worker/source request');
    } finally { await h.cancel(held.sent.operationId); held.release(); }

    // Pinned 4chan mixed video/image extraction and explicit image semantics.
    album();
    const fourchan = '/video https://boards.4chan.org/test/thread/7001';
    const first = await h.submit(fourchan);
    await h.succeeded(first.operationId);
    assert.equal((await h.request(first.operationId)).params.engine, 'auto_gallery_v1');
    assert.deepEqual(botApi.lastGroupBytes, expected.map(x => x.bytes));
    await h.replay(fourchan, first);
    const image = await h.submit('/image https://boards.4chan.org/test/thread/7001');
    await h.succeeded(image.operationId);
    assert.deepEqual(botApi.lastGroupBytes, expected.map(x => x.bytes), 'explicit image retains video in extractor order');
    const sends = botApi.sendCalls;
    const broken = await h.submit('/video https://boards.4chan.org/test/thread/7002');
    await waitFor(async () => (await queryJob(broken.operationId)).startsWith('failed|'), 'missing gallery video', 45000);
    await assertNoPublish(broken.operationId, sends);

    const meta = await mixed('PRMeta');
    assert.deepEqual(providerSource.events('PRMeta').map(e => e.stage), ['primary-api', 'gallery-api', 'gallery-media', 'gallery-media']);
    assert.equal(providerSource.events('PRMeta', 'primary-api').length, 1);
    assert.equal(providerSource.events('PRMeta', 'primary-media').length, 0);
    assert.equal(providerSource.events('PRMeta', 'gallery-api').length, 1);
    assert(providerSource.events('PRMeta', 'gallery-media').length === 2);
    assert(providerSource.events('PRMeta').filter(e => e.stage.endsWith('-api')).every(e => e.authorized), 'both providers receive the frozen session credential');

    const invalidJson = await mixed('PRJson');
assert.deepEqual(providerSource.events('PRJson').map(e => e.stage), ['primary-api', 'gallery-api', 'gallery-media', 'gallery-media']);

const download = await mixed('PRDownload');
    assert.deepEqual(providerSource.events('PRDownload').map(e => e.stage), ['primary-api', 'primary-media', 'gallery-api', 'gallery-media', 'gallery-media']);
    assert.equal(providerSource.events('PRDownload', 'primary-media').length, 1, 'primary download must actually start and return a truncated body');
    assert.equal(providerSource.events('PRDownload', 'gallery-api').length, 1);
    for (const admitted of [meta, invalidJson, download]) {
      const retained = await metadata(admitted.operationId);
      assert(retained.every(item => item.metadata.description === 'ordinary gallery evidence'));
      assert(!JSON.stringify(retained).includes('primary-only'), 'failed primary metadata cannot contaminate retained cache');
      assert(!JSON.stringify(retained).includes('sensitivecontent'), 'failed primary NSFW evidence cannot spoil the safe fallback');
    }
    await h.replay(command('PRDownload'), download);
    await h.reject(`/image ${url('PRDownload')}`, 'This request conflicts with an existing operation.', download.messageId);

    botApi.setExpectedAlbum(null);
    botApi.setExpectedUpload({ method: 'sendvideo', filename: 'media.mp4', mimeType: 'video/mp4', preserveMdat: false });
    const primary = await h.submit(command('PRSuccess'));
    await h.succeeded(primary.operationId);
    assert.deepEqual(botApi.lastUploadedBytes, primaryBytes);
    assert.equal(providerSource.events('PRSuccess', 'primary-api').length, 1);
    assert.equal(providerSource.events('PRSuccess', 'primary-media').length, 1);
    assert.equal(providerSource.events('PRSuccess', 'gallery-api').length, 0, 'successful primary suppresses all gallery metadata/download requests');
    assert.equal(mediaCalls(primary.callStart)[0].has_spoiler, true, 'successful primary retains its own NSFW source metadata');

    const both = await failed('PRBoth');
    assert.equal(providerSource.events('PRBoth', 'primary-api').length, 1);
    assert.equal(providerSource.events('PRBoth', 'gallery-api').length, 1);
    assert.equal(providerSource.events('PRBoth', 'gallery-media').length, 0);
    await h.replay(command('PRBoth'), both);
    const limit = await failed('PRLimit');
    assert.equal(providerSource.events('PRLimit', 'primary-media').length, 1);
    assert.equal(providerSource.events('PRLimit', 'gallery-api').length, 0, 'primary budget denial cannot activate fallback');
    const confinement = await failed('PRRedirect');
    assert(providerSource.events('PRRedirect', 'primary-media').length >= 1, 'primary must encounter the real forbidden redirect');
    assert.equal(providerSource.events('PRRedirect', 'gallery-api').length, 0, 'forbidden redirect cannot activate fallback');
    await failed('PRItems');
    assert.equal(providerSource.events('PRItems', 'gallery-api').length, 1);
    assert(providerSource.events('PRItems', 'gallery-media').length >= 20, 'item limit is reached through actual gallery downloads');
    assert.equal(await cacheRows(limit.operationId), '0');
    assert.equal(await cacheRows(confinement.operationId), '0');

    // Queued admission persists one routing snapshot and one charge through process restart.
    album();
    const restartHeld = await blocker();
    const beforeCounters = await counters();
    const restart = await h.submit(command('PRRestart'), privateRoute);
    assert((await queryJob(restart.operationId)).startsWith('queued|'));
    const frozen = await h.request(restart.operationId);
    assert.equal(frozen.network.cookie.id, actorCookie);
    assert.equal(frozen.params.engine, 'auto_fallback_v1');
    const afterAdmission = await counters();
    assert.notEqual(afterAdmission, beforeCounters);
    assert.equal(await fixtureSql(`SELECT count(*) FROM rate_limits WHERE user_id=${actor} AND count=1`), '3', 'one queued operation charges exactly one unit in each window');
    await h.replay(command('PRRestart'), restart, privateRoute);
    assert.equal(await counters(), afterAdmission);
    await restartApp();
    restartHeld.release();
    await h.succeeded(restart.operationId);
    assert.deepEqual(await h.request(restart.operationId), frozen);
    assert.equal(await fixtureSql(`SELECT attempt FROM runtime_operations WHERE id='${restart.operationId}'`), '1');
    const receipts = await fixtureSql(`SELECT * FROM runtime_delivery_units WHERE operation_id='${restart.operationId}' ORDER BY ordinal`);
    const fetched = providerSource.events('PRRestart').length;
    await h.replay(command('PRRestart'), restart, privateRoute);
    await restartApp();
    await h.replay(command('PRRestart'), restart, privateRoute);
    assert.equal(providerSource.events('PRRestart').length, fetched);
    assert.equal(await counters(), afterAdmission, 'same-message replay/restart cannot consume another charge');
    assert.equal(await fixtureSql(`SELECT * FROM runtime_delivery_units WHERE operation_id='${restart.operationId}' ORDER BY ordinal`), receipts);
    const cached = await mixed('PRRestart', privateRoute);
    assert.equal(mediaCalls(cached.callStart)[0].cached, true);
    assert.equal(providerSource.events('PRRestart').length, fetched, 'new admission reuses complete fallback cache after restart');
    assert.equal(await fixtureSql(`SELECT count(*) FROM rate_limits WHERE user_id=${actor} AND count=2`), '3', 'only a genuinely new operation charges again');

    // Delivery-time credential revocation must not select the still-permitted pool cookie.
    const poolCookie = await insertCookie(42, true);
    await fixtureSql(`UPDATE dl_user_settings SET use_pool_cookies=1 WHERE user_id=${actor}`);
    const releaseCookie = providerSource.hold('PRCookie', 'gallery-media');
    const cookieSends = botApi.sendCalls;
    const cookie = await h.submit(command('PRCookie'), privateRoute);
    await waitFor(() => providerSource.events('PRCookie', 'gallery-media').some(e => e.path.endsWith('gallery.mp4')), 'fallback reached final held media before credential removal');
    const cookieFrozen = await h.request(cookie.operationId);
    assert.equal(cookieFrozen.network.cookie.id, actorCookie);
    await fixtureSql(`DELETE FROM cookies WHERE id=${actorCookie}`);
    releaseCookie();
    await waitFor(async () => (await queryJob(cookie.operationId)).startsWith('failed|'), 'frozen cookie removal blocks delivery');
    assert.deepEqual(await h.request(cookie.operationId), cookieFrozen);
    assert.equal(await fixtureSql(`SELECT count(*) FROM cookies WHERE id=${poolCookie} AND is_valid=1`), '1');
    await assertNoPublish(cookie.operationId, cookieSends);
    actorCookie = await insertCookie(actor);
    await fixtureSql(`UPDATE dl_user_settings SET use_pool_cookies=0 WHERE user_id=${actor}`);

    const releaseRole = providerSource.hold('PRRole', 'gallery-media');
    const roleSends = botApi.sendCalls;
    const role = await h.submit(command('PRRole'), privateRoute);
    await waitFor(() => providerSource.events('PRRole', 'gallery-media').some(e => e.path.endsWith('gallery.mp4')), 'fallback media before role revocation');
    await fixtureSql(`UPDATE users SET role='restricted' WHERE id=${actor}`);
    releaseRole();
    await waitFor(async () => (await queryJob(role.operationId)).startsWith('failed|'), 'role revocation blocks fallback delivery');
    await assertNoPublish(role.operationId, roleSends);
    await fixtureSql(`UPDATE users SET role=${literal(oldRole)} WHERE id=${actor}`);

    // Queued authorization revocation also prevents either provider from starting.
    const queueHeld = await blocker();
    const queuedSends = botApi.sendCalls;
    const queued = await h.submit(command('PRRole') + '?queued=1', privateRoute);
    assert((await queryJob(queued.operationId)).startsWith('queued|'));
    const roleEvents = providerSource.events('PRRole').length;
    await fixtureSql(`UPDATE users SET role='restricted' WHERE id=${actor}`);
    await h.cancel(queueHeld.sent.operationId);
    queueHeld.release();
    await waitFor(async () => (await queryJob(queued.operationId)).startsWith('failed|'), 'revoked queued authorization prevents worker start');
    assert.equal(providerSource.events('PRRole').length, roleEvents);
    await assertNoPublish(queued.operationId, queuedSends);
    await fixtureSql(`UPDATE users SET role=${literal(oldRole)} WHERE id=${actor}`);

    const releasePrimaryCancel = providerSource.hold('PRAbort', 'primary-media');
    const primaryCancelSends = botApi.sendCalls;
    const aborted = await h.submit(command('PRAbort'));
    await waitFor(() => providerSource.events('PRAbort', 'primary-media').length === 1, 'cancellation during real primary download');
    await h.cancel(aborted.operationId);
    releasePrimaryCancel();
    await assertNoPublish(aborted.operationId, primaryCancelSends);
    assert.equal(providerSource.events('PRAbort', 'gallery-api').length, 0, 'primary cancellation never activates gallery');
    assert.equal(providerSource.events('PRBlocker', 'gallery-api').length, 0, 'repeated held-primary cancellation/restart never activates gallery');

    const releaseCancel = providerSource.hold('PRCancel', 'gallery-api');
    const cancelSends = botApi.sendCalls;
    const cancelled = await h.submit(command('PRCancel'));
    await waitFor(() => providerSource.events('PRCancel', 'gallery-api').length === 1, 'cancellation during gallery metadata fallback');
    await h.cancel(cancelled.operationId);
    releaseCancel();
    await assertNoPublish(cancelled.operationId, cancelSends);
    assert.equal(providerSource.events('PRCancel', 'gallery-media').length, 0);

    // Real gallery prose drives unchanged Rust NSFW policy for fresh, cached and revoked delivery.
    await restartGroups(true);
    await fixtureSql(`UPDATE groups SET nsfw_allowed=0 WHERE id=${group}`);
    const nsfw = await mixed('PRNsfw', privateRoute, true);
    const nsfwMetadata = await metadata(nsfw.operationId);
    assert(nsfwMetadata.every(item => item.metadata.description === 'sensitivecontent gallery evidence'));
    const nsfwFetched = providerSource.events('PRNsfw').length;
    const deniedSends = botApi.sendCalls;
    const denied = await h.submit(command('PRNsfw'), groupRoute);
    await waitFor(async () => (await queryJob(denied.operationId)).startsWith('failed|'), 'cached NSFW fallback group refusal');
    assert.equal(botApi.sendCalls, deniedSends);
    assert.equal(providerSource.events('PRNsfw').length, nsfwFetched);
    assert.equal(await fixtureSql(`SELECT count(*) FROM runtime_delivery_units WHERE operation_id='${denied.operationId}'`), '0');
    // This is a previously successful actor cache, not a newly published denial cache.
    assert.equal(await cacheRows(nsfw.operationId), '1');
    await fixtureSql(`UPDATE groups SET nsfw_allowed=1 WHERE id=${group}`);
    const allowed = await mixed('PRNsfw', groupRoute, false);
    assert.equal(mediaCalls(allowed.callStart)[0].cached, true);
    assert.equal(mediaCalls(allowed.callStart)[0].message_thread_id, 18);
    assert.equal(providerSource.events('PRNsfw').length, nsfwFetched);
    await restartApp();
    await fixtureSql(`UPDATE groups SET nsfw_allowed=0 WHERE id=${group}`);
    const again = await h.submit(command('PRNsfw'), groupRoute);
    await waitFor(async () => (await queryJob(again.operationId)).startsWith('failed|'), 'NSFW policy and cached metadata survive restart');
    assert.equal(providerSource.events('PRNsfw').length, nsfwFetched);
    const privateCached = await mixed('PRNsfw', privateRoute, true);
    assert.equal(mediaCalls(privateCached.callStart)[0].cached, true);

    await fixtureSql(`UPDATE groups SET nsfw_allowed=1 WHERE id=${group}`);
    const releasePolicy = providerSource.hold('PRPolicy', 'gallery-media');
    const policySends = botApi.sendCalls;
    const policy = await h.submit(command('PRPolicy'), groupRoute);
    await waitFor(() => providerSource.events('PRPolicy', 'gallery-media').some(e => e.path.endsWith('gallery.mp4')), 'gallery media before NSFW group-policy revocation');
    await fixtureSql(`UPDATE groups SET nsfw_allowed=0 WHERE id=${group}`);
    releasePolicy();
    await waitFor(async () => (await queryJob(policy.operationId)).startsWith('failed|'), 'revoked NSFW group policy blocks fresh fallback delivery');
    await assertNoPublish(policy.operationId, policySends);
    assert(!JSON.stringify(botApi.calls).includes(cookieSecret));
    await context.assertWorkspaceClean();
    return 'provider-routing native table/deceptive hosts/VK-query; pinned 4chan and Instagram metadata/download fallback, primary suppression, both-fail isolation, mixed video, image override, budget/confinement/cancellation, running/queued credential/role policy checks, one-charge restart replay/cache, and NSFW fresh/cache/topic/revocation';
  } catch (error) {
    // Fixed synthetic IDs/stages only; never include request cookies, credentials or headers.
    console.error('Provider fixture trace:', JSON.stringify(providerSource.trace));
    console.error('Provider fixture request paths:', JSON.stringify([...gallerySource.paths].map(value => new URL(value, 'https://fixture.invalid').pathname)));
    throw error;
  } finally {
    providerSource.releaseAll();
    botApi.setExpectedAlbum(null);
    botApi.setExpectedUpload(null);
    await fixtureSql(`UPDATE users SET role=${literal(oldRole)} WHERE id=${actor}`);
    await fixtureSql(`UPDATE groups SET nsfw_allowed=${Number(oldGroupPolicy)} WHERE id=${group}`);
    if (cookieIds.length) await fixtureSql(`DELETE FROM cookies WHERE id IN (${cookieIds.join(',')})`);
    if (ruleIds.length) await fixtureSql(`DELETE FROM nsfw_keywords WHERE id IN (${ruleIds.join(',')})`);
    for (const [id, old] of previous) {
      if (old) await fixtureSql(`UPDATE dl_user_settings SET ${Object.entries(old).map(([key, value]) => `${key}=${literal(value)}`).join(',')} WHERE user_id=${id}`);
      else await fixtureSql(`DELETE FROM dl_user_settings WHERE user_id=${id}`);
    }
    await fixtureSql(`DELETE FROM rate_limits WHERE user_id=${actor}`);
    for (const row of oldCounters) await fixtureSql(`INSERT INTO rate_limits(user_id,window,count,reset_at) VALUES(${actor},${literal(row.window)},${row.count},${row.reset_at})`);
    if (!botApi.error && !gallerySource.error) {
      await restartGroups(false);
      await restartGallery(false);
    }
  }
}
