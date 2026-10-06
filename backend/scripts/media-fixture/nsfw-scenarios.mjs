import assert from 'node:assert/strict';
import { readFile } from "node:fs/promises";
import { withRussianPreference } from "./language-fixture.mjs";
import { createHmac } from 'node:crypto';
import { commandHarness } from './command-harness.mjs';

async function verifyNsfwAdmin({ appHttpPort, telegramToken, fixtureSql, restartGroups }) {
  const base = `http://127.0.0.1:${appHttpPort}/api/v1`;
  async function request(path, { token, method = 'GET', body, status = 200, etag } = {}) {
    const response = await fetch(base + path, {
      method,
      headers: { ...(token ? { authorization: `Bearer ${token}` } : {}), ...(body === undefined ? {} : { 'content-type': 'application/json' }), ...(etag ? { 'if-match': etag } : {}) },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    assert.equal(response.status, status, `${method} ${path}`);
    if (status !== 204) assert.equal(response.headers.get('cache-control'), 'no-store');
    const text = await response.text();
    return { value: text ? JSON.parse(text) : null, etag: response.headers.get('etag') };
  }
  async function login(id) {
    const auth_date = String(Math.floor(Date.now() / 1000));
    const user = JSON.stringify({ id, first_name: 'NSFW fixture' });
    const key = createHmac('sha256', 'WebAppData').update(telegramToken).digest();
    const hash = createHmac('sha256', key).update(`auth_date=${auth_date}\nuser=${user}`).digest('hex');
    const result = await request('/auth/token', { method: 'POST', body: { init_data: new URLSearchParams({ auth_date, user, hash }).toString() } });
    return result.value.access_token;
  }
  const id = 8_500_000_000_001;
  const owner = await login(42), ordinary = await login(id);
  const created = [];
  const root = '/dl/nsfw';
  const mutate = (path, method, body, status = 200, token = owner, etag) => request(root + path, { token, method, body, status, etag });
  async function create(kind, body) {
    const { value } = await mutate(`/${kind}`, 'POST', body, 201, ordinary);
    created.push([kind, value.id]);
    return value;
  }
  try {
    await request(root + '/domains', { status: 401 });
    await request(root + '/keywords', { token: ordinary, status: 403 });
    await request(`/users/${id}`, { token: owner, method: 'PATCH', body: { role: 'moderator' } });
    const initial = await request(root + '/domains', { token: ordinary });
    assert.match(initial.etag, /^"\d+"$/);
    const domain = await create('domains', { domain: 'WWW.Nsfw-Fixture.Example', note: 'native NSFW API acceptance' });
    assert.equal(domain.domain, 'nsfw-fixture.example');
    assert.equal(typeof domain.created_at, 'string');
    await mutate(`/domains/${domain.id}`, 'PATCH', { note: 'stale revision' }, 409, owner, initial.etag);
    const current = await request(root + '/domains', { token: ordinary });
    await mutate(`/domains/${domain.id}`, 'PATCH', { note: 'updated fixture' }, 200, ordinary, current.etag);
    const advanced = await request(root + '/domains', { token: ordinary });
    assert(Number(advanced.etag.replaceAll('"', '')) > Number(current.etag.replaceAll('"', '')));
    const keyword = await create('keywords', { keyword: 'fixture content', note: 'native NSFW API acceptance' });
    const domainMatch = await mutate('/check', 'POST', { url: 'https://sub.nsfw-fixture.example/video' });
    assert.equal(domainMatch.value.is_nsfw, true);
    assert.equal(domainMatch.value.matched_domain, domain.domain);
    const keywordMatch = await mutate('/check', 'POST', { url: 'https://safe.example/fixture-content' });
    assert.deepEqual(keywordMatch.value.matched_keywords, ['fixture content']);
    assert.equal((await mutate('/check', 'POST', { url: 'https://safe.example/notfixture-contentual' })).value.is_nsfw, false);
    await mutate('/check', 'POST', { url: 'https://safe.example/', metadata: { title: 'fixture content' } }, 422);
    await mutate('/domains', 'POST', { domain: 'bad/path' }, 422);
    await mutate('/keywords', 'POST', { keyword: keyword.keyword }, 409);
    const exported = await request(root + '/export', { token: owner });
    assert(exported.value.domains.some(row => row.domain === domain.domain && row.note === 'updated fixture'));
    assert(exported.value.keywords.some(row => row.keyword === keyword.keyword));
    const imported = await mutate('/import', 'POST', { domains: [{ domain: domain.domain }], keywords: [{ keyword: keyword.keyword }] });
    assert.deepEqual(imported.value, { domains_added: 0, keywords_added: 0 });
    const newRules = { domains: [{ domain: 'imported-nsfw-fixture.example' }], keywords: [{ keyword: 'imported fixture content' }] };
    const added = await mutate('/import', 'POST', newRules, 200, ordinary);
    assert.deepEqual(added.value, { domains_added: 1, keywords_added: 1 });
    for (const [kind, field] of [['domains', 'domain'], ['keywords', 'keyword']]) {
      const rows = (await request(root + '/' + kind, { token: ordinary })).value;
      const importedRule = rows.find(row => row[field] === newRules[kind][0][field]);
      assert(importedRule, `imported ${kind} row must be durable`);
      created.push([kind, importedRule.id]);
    }
    assert.equal((await mutate('/check', 'POST', { url: 'https://imported-nsfw-fixture.example/video' })).value.is_nsfw, true);
    await mutate(`/domains/${domain.id}`, 'PATCH', { domain: 'renamed-nsfw-fixture.example' }, 200, ordinary);
    assert.equal((await mutate('/check', 'POST', { url: 'https://sub.nsfw-fixture.example/video' })).value.is_nsfw, false);
    assert.equal((await mutate('/check', 'POST', { url: 'https://sub.renamed-nsfw-fixture.example/video' })).value.is_nsfw, true);
    await mutate(`/keywords/${keyword.id}`, 'PATCH', { keyword: 'renamed fixture content' }, 200, ordinary);
    assert.equal((await mutate('/check', 'POST', { url: 'https://safe.example/fixture-content' })).value.is_nsfw, false);
    assert.equal((await mutate('/check', 'POST', { url: 'https://safe.example/renamed-fixture-content' })).value.is_nsfw, true);
    await request(`/users/${id}`, { token: owner, method: 'PATCH', body: { role: 'user' } });
    await request(root + '/domains', { token: ordinary, status: 403 });
    await mutate(`/keywords/${keyword.id}`, 'PATCH', { note: 'revoked moderator' }, 403, ordinary);
    await restartGroups(true);
    assert.equal((await mutate('/check', 'POST', { url: 'https://sub.renamed-nsfw-fixture.example/video' })).value.is_nsfw, true);
  } finally {
    for (const [kind, ruleId] of created) await mutate(`/${kind}/${ruleId}`, 'DELETE', undefined, 204);
    await fixtureSql(`DELETE FROM users WHERE id=${id}`);
  }
}

export async function runNsfwScenarios(context) {
  const { botApi, sourceFixture, sourcePort, fixtureSql, queryJob, waitFor, restartGroups } = context;
  const h = commandHarness(context, 8100);
  const actor = 8000000000011, group = -8000000000011;
  const origin = `http://127.0.0.1:${sourcePort}`;
  const privateRoute = { actor, chat: actor };
  const groupRoute = { actor, chat: group, thread: 18 };
  const media = start => botApi.calls.slice(start).filter(call => ['sendvideo', 'sendphoto', 'sendaudio', 'senddocument', 'sendmediagroup'].includes(call.method));
  let nextMessageId = 8300;
  const ruleIds = [];
  const oldGroupPolicy = await fixtureSql(`SELECT nsfw_allowed FROM groups WHERE id=${group}`);
  const oldSettings = await fixtureSql(`SELECT nsfw_blur FROM dl_user_settings WHERE user_id=${actor}`);
  async function addKeyword(keyword) {
    const id = Number(await fixtureSql(`INSERT INTO nsfw_keywords(keyword,note,created_at) VALUES ('${keyword}','nsfw acceptance',unixepoch('now')*1000000) RETURNING id`));
    assert(Number.isSafeInteger(id) && id > 0);
    ruleIds.push(['nsfw_keywords', id]);
    return id;
  }
  async function addDomain(domain) {
    const id = Number(await fixtureSql(`INSERT INTO nsfw_domains(domain,note,created_at) VALUES ('${domain}','nsfw acceptance',unixepoch('now')*1000000) RETURNING id`));
    assert(Number.isSafeInteger(id) && id > 0);
    ruleIds.push(['nsfw_domains', id]);
    return id;
  }
  async function succeeded(command, route = privateRoute) {
    const sent = await h.submit(command, route);
    await h.succeeded(sent.operationId);
    const calls = media(sent.callStart);
    assert.equal(calls.length, 1, 'successful NSFW scenario performs one delivery');
    return { ...sent, call: calls[0] };
  }
  async function blocked(command, { messageId = nextMessageId++, ...route } = groupRoute) {
    const start = botApi.calls.length;
    const sends = botApi.sendCalls;
    botApi.enqueue(actor, route.chat, command, messageId, 'en', { thread: route.thread });
    await waitFor(() => botApi.calls.slice(start).some(call =>
      ['sendmessage', 'editmessagetext'].includes(call.method) && Number(call.data.chat_id) === route.chat && /NSFW/i.test(call.data.text)), 'localized NSFW policy refusal', 30000);
    const ids = botApi.calls.slice(start).flatMap(call => (call.data?.text ?? '').match(/[0-9a-f]{8}-[0-9a-f-]{27}/ig) ?? []);
    if (ids.length) await waitFor(async () => (await queryJob(ids[0])).startsWith('failed|'), 'NSFW operation terminal failure');
    assert.equal(botApi.sendCalls, sends, 'blocked content cannot issue any Telegram media send');
    return { messageId, operationId: ids[0] };
  }
  await restartGroups(true);
  try {
    await verifyNsfwAdmin(context);
    await fixtureSql(`UPDATE groups SET nsfw_allowed=0 WHERE id=${group}`);
    await fixtureSql(`INSERT INTO dl_user_settings(user_id,created_at,updated_at) VALUES (${actor},unixepoch('now')*1000000,unixepoch('now')*1000000) ON CONFLICT(user_id) DO UPDATE SET nsfw_blur=1`);

    const metadataCommand = `/video ${origin}/content-page.html?case=classification-cache`;
    const clean = await succeeded(metadataCommand);
    assert.equal(clean.call.has_spoiler, false, 'unclassified private media is not blurred');
    const cachedSourceRequests = sourceFixture.mediaRequests;
    const metadataRule = await addKeyword('sensitivecontent');
    await blocked(metadataCommand);
    const privateCached = await succeeded(metadataCommand);
    assert.equal(privateCached.call.cached, true, 'metadata policy uses confirmed cached media without redownload');
    assert.equal(privateCached.call.has_spoiler, true, 'new rules classify retained metadata on cached private delivery');

    await fixtureSql(`UPDATE groups SET nsfw_allowed=1 WHERE id=${group}`);
    const allowedCached = await succeeded(metadataCommand, groupRoute);
    assert.equal(allowedCached.call.cached, true);
    assert.equal(allowedCached.call.has_spoiler, false, 'allowed group media does not inherit private blur');
    await fixtureSql(`UPDATE groups SET nsfw_allowed=0 WHERE id=${group}`);
    await blocked(metadataCommand);
    await restartGroups(true);
    await blocked(metadataCommand);
    await fixtureSql(`DELETE FROM nsfw_keywords WHERE id=${metadataRule}`);
    const reclassified = await succeeded(metadataCommand, groupRoute);
    assert.equal(reclassified.call.cached, true, 'removed rules take effect without replacing the cached file');
    assert.equal(sourceFixture.mediaRequests, cachedSourceRequests, 'cached classification across rule changes and restart cannot redownload media');

    const urlRule = await addKeyword('fixtureadult');
    const urlCommand = `/video ${origin}/media?fixtureadult=1&case=url-classification`;
    const denied = await blocked(urlCommand);
    const operationCount = await fixtureSql('SELECT count(*) FROM runtime_operations');
    const replayed = await h.submit(urlCommand, { ...groupRoute, messageId: denied.messageId });
    assert.equal(replayed.operationId, denied.operationId, 'replayed denial returns its original operation');
    assert.equal(await fixtureSql('SELECT count(*) FROM runtime_operations'), operationCount, 'replayed denial cannot create a second operation');
    const jobCallStart = botApi.calls.length;
    botApi.enqueue(actor, group, `/job ${denied.operationId}`, 2_010_001, 'en', { thread: 18 });
    await waitFor(() => botApi.calls.slice(jobCallStart).some(call => call.method === 'sendmessage' && /NSFW|sensitive content|18\+/i.test(call.data.text)), 'persisted NSFW reason from job status');
    const privateUpload = await succeeded(urlCommand);
    assert.equal(privateUpload.call.has_spoiler, true, 'fresh private NSFW video has spoiler');
    const beforeBlurChange = sourceFixture.mediaRequests;
    await fixtureSql(`UPDATE dl_user_settings SET nsfw_blur=0 WHERE user_id=${actor}`);
    const noBlur = await succeeded(urlCommand);
    assert.equal(noBlur.call.cached, true, 'private blur setting changes do not require redownload');
    assert.equal(sourceFixture.mediaRequests, beforeBlurChange, 'private blur opt-out reuses retained cache metadata');
    assert.equal(noBlur.call.has_spoiler, false, 'explicit private blur opt-out is honored');
    await fixtureSql(`DELETE FROM nsfw_keywords WHERE id=${urlRule}`);

    const domainRule = await addDomain('127.0.0.1');
    await blocked(`/video ${origin}/media?case=domain-classification`);
    await fixtureSql(`DELETE FROM nsfw_domains WHERE id=${domainRule}`);

    await addKeyword('revokedcontent');
    await fixtureSql(`UPDATE groups SET nsfw_allowed=1 WHERE id=${group}`);
    sourceFixture.closeProgressGate();
    const revoked = await h.submit(`/video ${origin}/progress.mp4?case=revokedcontent`, groupRoute);
    await waitFor(() => botApi.calls.slice(revoked.callStart).some(call => call.method === 'editmessagetext' && /Downloading\. \d+%/.test(call.data.text)), 'NSFW download before policy revocation');
    const sendsBeforeRevocation = botApi.sendCalls;
    await fixtureSql(`UPDATE groups SET nsfw_allowed=0 WHERE id=${group}`);
    sourceFixture.releaseProgressResponses();
    await waitFor(async () => (await queryJob(revoked.operationId)).startsWith('failed|'), 'revoked NSFW delivery blocked', 30000);
    assert.equal(botApi.sendCalls, sendsBeforeRevocation, 'policy revocation after admission blocks fresh delivery');
    await context.assertWorkspaceClean();
  } finally {
    sourceFixture.releaseProgressResponses();
    for (const [table, id] of ruleIds) await fixtureSql(`DELETE FROM ${table} WHERE id=${id}`);
    await fixtureSql(`UPDATE groups SET nsfw_allowed=${Number(oldGroupPolicy)} WHERE id=${group}`);
    if (oldSettings === '') await fixtureSql(`DELETE FROM dl_user_settings WHERE user_id=${actor}`);
    else await fixtureSql(`UPDATE dl_user_settings SET nsfw_blur=${Number(oldSettings)} WHERE user_id=${actor}`);
    await restartGroups(false);
  }
  await runNsfwMarkScenarios(context);
  return 'NSFW domain/URL/metadata rules, fresh/cache blur, rule changes, group revocation, durable one-shot mark consumption, duplicate/replayed updates, queued restart and nonsticky cached delivery passed';
}


// Disposable fixture only. No classification is supplied by this controller.
async function runNsfwMarkScenarios(context) {
  const { botApi, fixtureSql, sourcePort, sourceFixture, queryJob, waitFor, restartApp } = context;
  const h = commandHarness(context, 22000);
  const stateSql = await readFile(new URL('./nsfw-state.sql', import.meta.url), 'utf8');
  const cleanupSql = await readFile(new URL('./nsfw-cleanup.sql', import.meta.url), 'utf8');
  const settingsSql = await readFile(new URL('./nsfw-settings-snapshot.sql', import.meta.url), 'utf8');
  const oldSettings = await fixtureSql(settingsSql);
  let next = 23000;
  async function command(text, prefix, messageId = next++) {
    const start = botApi.calls.length;
    botApi.enqueue(42, 42, text, messageId);
    return waitFor(() => botApi.calls.slice(start).find(c => c.method === 'sendmessage' && c.data.text.startsWith(prefix)), `NSFW ${text}`);
  }
  const url = `http://127.0.0.1:${sourcePort}/media.mp4?case=nsfw-mark`;
  try {
    await command('/settings send_as_file false', 'Download settings');
    await command('/settings quality best', 'Download settings');
    await command('/settings codec avc1', 'Download settings');
    await command('/settings container mp4', 'Download settings');
    await command('/nsfw on', 'Private NSFW blur:');
    const menu = await command('/nsfw menu', 'Private NSFW blur:');
    const off = menu.data.reply_markup.inline_keyboard.flat().find(b => b.callback_data.startsWith('nsfw:off:'));
    assert(off);
    await command('/nsfw off', 'Private NSFW blur:');
    const start = botApi.calls.length;
    const id = botApi.enqueueCallback(42, 42, menu.messageId, off.callback_data);
    const ack = await waitFor(() => botApi.calls.slice(start).find(c => c.method === 'answercallbackquery' && c.data.callback_query_id === id), 'stale NSFW button');
    assert.equal(ack.data.show_alert, true);
    await command('/nsfw on', 'Private NSFW blur:');
    await withRussianPreference(fixtureSql, () => command('/nsfw menu', 'Размытие NSFW'));
    await command('/nsfw reload', 'NSFW rules are current.');
    const markId = next++;
    await command('/nsfw mark', 'The next admitted URL', markId);
    assert.equal(await fixtureSql(stateSql), '1');
    await restartApp();
    assert.equal(await fixtureSql(stateSql), '1', 'arm survives restart before admission');
    const first = await h.execute('forced clean source', `/video ${url}`, 'sendvideo',
      { video: true, audio: true }, { filename: 'media.mp4', mimeType: 'video/mp4' });
    assert.equal((await h.request(first.operationId)).force_nsfw, true);
    const forcedSend = botApi.calls.slice(first.callStart).find(c => c.method === 'sendvideo');
    assert.equal(String(forcedSend.has_spoiler), 'true');
    const fetched = sourceFixture.mediaRequests;
    assert.equal(await fixtureSql(stateSql), '0');
    await command('/nsfw mark', 'The next admitted URL', markId);
    assert.equal(await fixtureSql(stateSql), '0', 'old mark update cannot re-arm');
    await h.replay(`/video ${url}`, first);
    await restartApp();
    assert.equal((await h.request(first.operationId)).force_nsfw, true, 'admitted intent remains immutable');
    const second = await h.submit(`/video ${url}`);
    await h.succeeded(second.operationId);
    assert.equal((await h.request(second.operationId)).force_nsfw, undefined);
    const cleanSend = botApi.calls.slice(second.callStart).find(c => c.method === 'sendvideo');
    assert(cleanSend);
    assert.equal(cleanSend.cached, true, 'unmarked repeat uses the marked operation cache entry');
    assert.equal(sourceFixture.mediaRequests, fetched, 'unmarked cache hit cannot refetch media');
    assert.notEqual(String(cleanSend.has_spoiler), 'true', 'forced classification cannot stick in shared cache');
    sourceFixture.closeProgressGate();
    const blocker = await h.submit('/video http://127.0.0.1:' + sourcePort + '/progress.mp4?case=nsfw-blocker');
    await waitFor(() => botApi.calls.slice(blocker.callStart).some(c => c.method === 'editmessagetext' && /Downloading\. \d+%/.test(c.data.text)), 'NSFW queued restart blocker');
    await command('/nsfw mark', 'The next admitted URL');
    const queued = await h.submit('/video ' + url + '-queued');
    assert((await queryJob(queued.operationId)).startsWith('queued|'));
    assert.equal((await h.request(queued.operationId)).force_nsfw, true);
    await restartApp();
    assert.equal((await h.request(queued.operationId)).force_nsfw, true, 'queued marked request survives process restart');
    sourceFixture.releaseProgressResponses();
    await h.succeeded(blocker.operationId);
    await h.succeeded(queued.operationId);
    const queuedSend = botApi.calls.slice(queued.callStart).find(c => c.method === 'sendvideo' && c.has_spoiler);
    assert(queuedSend, 'queued marked clean media still receives a spoiler after restart');
  } finally {
    sourceFixture.releaseProgressResponses();
    await fixtureSql(cleanupSql);
    if (oldSettings.trim()) {
      const old = JSON.parse(oldSettings);
      let restore = await readFile(new URL('./nsfw-settings-restore.sql', import.meta.url), 'utf8');
      for (const [key, value] of Object.entries(old)) {
        const literal = typeof value === 'number' ? String(value) : "'" + value.replaceAll("'", "''") + "'";
        restore = restore.replace('{' + key + '}', literal);
      }
      await fixtureSql(restore);
    } else await fixtureSql(await readFile(new URL('./nsfw-settings-delete.sql', import.meta.url), 'utf8'));
    botApi.setExpectedUpload(null);
  }
  return 'NSFW EN/RU menu, on/off revision fencing, reload, durable mark restart, duplicate update fencing, immutable admitted intent and clean cached delivery passed';
}
