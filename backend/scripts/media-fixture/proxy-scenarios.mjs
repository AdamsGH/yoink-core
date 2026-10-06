import assert from 'node:assert/strict';
import { setTimeout as sleep } from 'node:timers/promises';
import { commandHarness } from './command-harness.mjs';
import { withRussianPreference } from './language-fixture.mjs';

export async function runProxyScenarios(context) {
  const { botApi, fixtureSql, sourcePort, sourceFixture, proxyFixture, proxyUrl, waitFor, restartApp, queryJob, assertWorkspaceClean } = context;
  const h = commandHarness(context, 60000);
  const literal = value => value === null ? 'NULL' : typeof value === 'number' ? String(value) : `'${value.replaceAll("'", "''")}'`;
  const oldRow = await fixtureSql('SELECT * FROM dl_user_settings WHERE user_id=42');
  const saved = JSON.parse(await fixtureSql("SELECT json_object('proxy_enabled',proxy_enabled,'proxy_url',proxy_url,'quality',quality,'updated_at',updated_at) FROM dl_user_settings WHERE user_id=42") || 'null');
  const savedUser = JSON.parse(await fixtureSql("SELECT json_object('role',role,'ban_until',ban_until) FROM users WHERE id=42"));
  const state = async () => JSON.parse(await fixtureSql("SELECT json_object('enabled',proxy_enabled,'revision',updated_at) FROM dl_user_settings WHERE user_id=42"));
  const row = () => fixtureSql('SELECT * FROM dl_user_settings WHERE user_id=42');
  let next = 61000;
  const buttons = call => call.data.reply_markup?.inline_keyboard?.flat() ?? [];
  async function command(text = '/proxy', language = 'en') {
    const start = botApi.calls.length;
    botApi.enqueue(42, 42, text, next++, language);
    return waitFor(() => botApi.calls.slice(start).find(c => c.method === 'sendmessage' && Number(c.data.chat_id) === 42), 'proxy command response');
  }
  async function click(menu, data, { actor = 42, edit = false, ...options } = {}) {
    const start = botApi.calls.length;
    const id = botApi.enqueueCallback(actor, 42, menu.messageId, data, options);
    const ack = await waitFor(() => botApi.calls.slice(start).find(c => c.method === 'answercallbackquery' && c.data.callback_query_id === id), 'proxy callback acknowledgement');
    const changed = edit ? await waitFor(() => botApi.calls.slice(start).find(c => c.method === 'editmessagetext' && c.data.message_id === menu.messageId), 'proxy menu edit') : undefined;
    assert.equal(botApi.calls.slice(start).filter(c => c.method === 'answercallbackquery' && c.data.callback_query_id === id).length, 1);
    if (!edit) assert(!botApi.calls.slice(start).some(c => c.method === 'editmessagetext' && c.data.message_id === menu.messageId), 'rejected callback must not edit the menu');
    return { ack, changed };
  }
  async function toggle(enabled, language = 'en') {
    const menu = await command('/proxy', language);
    const button = buttons(menu).find(b => b.callback_data?.startsWith(`proxy:${enabled ? 'on' : 'off'}:`));
    assert(button, 'toggle must come from a served revision-bearing button');
    const before = await state();
    const result = await click(menu, button.callback_data, { edit: true, language });
    const after = await state();
    assert.equal(after.enabled, enabled ? 1 : 0);
    assert(after.revision > before.revision, 'proxy write advances the shared settings revision');
    return { menu, data: button.callback_data, ...result };
  }
  const video = url => `/video --height best --video-codec h264 --container mp4 --send-as-file false ${url}`;
  const media = { video: true, audio: true, durationMin: 0.8, durationMax: 1.5 };
  const contract = { filename: 'media.mp4', mimeType: 'video/mp4' };
  try {
    const initialOperations = await fixtureSql('SELECT count(*) FROM runtime_operations');
    const absent = await command();
    if (!saved) assert(!buttons(absent).some(b => b.callback_data?.startsWith('proxy:on:')), 'missing configuration must not offer enable');
    assert.equal(await row(), oldRow, 'opening a proxy menu must not create or modify settings');
    if (!saved) await command('/settings quality best');
    await fixtureSql('UPDATE dl_user_settings SET proxy_enabled=0,proxy_url=NULL WHERE user_id=42');
    const missing = await command();
    const missingRow = await row();
    const rejected = await click(missing, `proxy:on:${(await state()).revision}`);
    assert.equal(rejected.ack.data.show_alert, true);
    assert.equal(await row(), missingRow);

    await fixtureSql(`UPDATE dl_user_settings SET proxy_url=${literal(proxyUrl)} WHERE user_id=42`);
    const columns = (await fixtureSql("SELECT name FROM pragma_table_info('dl_user_settings') ORDER BY cid")).trim().split('\n');
    assert(columns.every(name => /^[a-z_]+$/.test(name)));
    const unchangedColumns = columns.filter(name => !['proxy_enabled', 'updated_at'].includes(name));
    const invariant = () => fixtureSql(`SELECT ${unchangedColumns.join(',')} FROM dl_user_settings WHERE user_id=42`);
    const baseline = await invariant();
    const enabled = await toggle(true);
    await toggle(false);
    const stable = await row();
    const stale = await click(enabled.menu, enabled.data);
    assert.equal(stale.ack.data.show_alert, true);
    assert.equal(await row(), stable, 'old enable callback cannot undo a later disable');
    assert.equal(await invariant(), baseline, 'proxy toggles preserve credentials and every unrelated preference');

    await restartApp();
    const staleAfterRestart = await click(enabled.menu, enabled.data);
    assert.equal(staleAfterRestart.ack.data.show_alert, true);
    assert.equal(await row(), stable);
    const russian = await withRussianPreference(fixtureSql, () => command('/proxy@YOINK_FIXTURE_BOT', 'en'));
    assert.match(russian.data.text, /прокси/i);
    const current = `proxy:on:${(await state()).revision}`;
    for (const data of ['proxy:on', 'proxy:off:-1', 'proxy:on:1:extra', 'proxy:secret:0', 'proxy:on:' + '9'.repeat(80)]) {
      assert.equal((await click(russian, data)).ack.data.show_alert, true);
    }
    for (const options of [{ actor: 43 }, { isBot: true }, { inaccessible: true },
      { messagePatch: { message_thread_id: 9 } },
      { messagePatch: { chat: { id: -42, type: 'group', title: 'Fixture' } } },
      { messagePatch: { from: { id: 999, is_bot: true, first_name: 'Foreign bot' } } }]) {
      await click(russian, current, options);
    }
    for (const text of ['/proxy on', '/proxy off extra', `/proxy ${proxyUrl}`]) {
      const invalid = await command(text);
      assert.match(invalid.data.text, /\/proxy/);
      assert.equal(buttons(invalid).length, 0);
    }
    const beforeSilent = botApi.calls.filter(c => c.method === 'sendmessage').length;
    for (const [actor, chat, text] of [[42, -10042, '/proxy'], [42, 42, '/proxy@other_bot']]) botApi.enqueue(actor, chat, text, next++);
    await command('/help');
    await sleep(300);
    assert.equal(botApi.calls.filter(c => c.method === 'sendmessage').length, beforeSilent + 1);
    await fixtureSql("UPDATE users SET ban_until=unixepoch('now')*1000000+600000000 WHERE id=42");
    assert.equal((await click(russian, current)).ack.data.show_alert, true);
    const denied = await command();
    assert.equal(denied.data.text, 'This operation is not allowed.');
    await fixtureSql(`UPDATE users SET ban_until=${literal(savedUser.ban_until)} WHERE id=42`);
    assert.equal(await row(), stable, 'invalid or unauthorized operations cannot mutate proxy settings');
    assert.equal(await fixtureSql('SELECT count(*) FROM runtime_operations'), initialOperations);

    const beforeSettings = await command();
    const oldProxyButton = buttons(beforeSettings).find(b => b.callback_data?.startsWith('proxy:on:')).callback_data;
    const auto = await fixtureSql('SELECT subs_auto FROM dl_user_settings WHERE user_id=42');
    await command(`/settings subs_auto ${auto === '1' ? 'true' : 'false'}`);
    const afterSettings = await row();
    assert.equal((await click(beforeSettings, oldProxyButton)).ack.data.show_alert, true);
    assert.equal(await row(), afterSettings, 'settings edit invalidates the old proxy button');
    const settingsMenu = await command('/settings');
    const settingsPage = (await click(settingsMenu, 'prefs:subs_auto:_menu', { edit: true })).changed;
    const oldSettingsButton = buttons(settingsPage).find(b => b.callback_data?.startsWith('prefs:subs_auto:false:')).callback_data;
    await withRussianPreference(fixtureSql, () => toggle(true, 'en'));
    const afterProxy = await row();
    assert.equal((await click(settingsMenu, oldSettingsButton)).ack.data.show_alert, true);
    assert.equal(await row(), afterProxy, 'proxy edit invalidates the old settings button');
    const url = `http://127.0.0.1:${sourcePort}/media?case=proxy-menu-route`;
    const hits = proxyFixture.authHits;
    const proxied = await h.execute('proxy menu enabled delivery', video(url), 'sendvideo', media, contract);
    assert(proxyFixture.authHits > hits, 'menu must enable real authenticated proxy traffic');
    const frozenProxy = await h.request(proxied.operationId);
    assert(frozenProxy.network.proxy, 'proxy selection is frozen at admission');
    await toggle(false);
    const directHits = proxyFixture.hits;
    const direct = await h.execute('proxy menu disabled delivery', video(url), 'sendvideo', media, contract);
    assert.equal(proxyFixture.hits, directHits, 'disabled proxy uses direct source traffic');
    assert.equal((await h.request(direct.operationId)).network.proxy, null);
    assert.deepEqual(await h.request(proxied.operationId), frozenProxy, 'toggle cannot rewrite the old admitted route');

    // Real callbacks change policy while one worker is running and another job is queued.
    // Both directions must fail closed, not silently reroute or upload stale work.
    for (const enable of [true, false]) {
      sourceFixture.closeGate();
      const sends = botApi.sendCalls;
      const blocker = await h.submit(video(`http://127.0.0.1:${sourcePort}/gated?case=proxy-menu-blocker-${enable}`));
      await waitFor(() => sourceFixture.gatedResponses.size > 0, 'proxy policy blocker reached source');
      const queuedFetches = sourceFixture.lateProxyQueuedFetches;
      const queued = await h.submit(video(`http://127.0.0.1:${sourcePort}/media?case=late-proxy-queued&menu=${enable}`));
      assert((await queryJob(queued.operationId)).startsWith('queued|'));
      const frozen = await h.request(queued.operationId);
      assert.equal(Boolean(frozen.network.proxy), !enable);
      const oldHits = proxyFixture.hits;
      await toggle(enable);
      assert.deepEqual(await h.request(queued.operationId), frozen);
      sourceFixture.releaseGatedResponses();
      await waitFor(async () => (await queryJob(blocker.operationId)).startsWith('failed|') && (await queryJob(queued.operationId)).startsWith('failed|'), 'proxy policy invalidated running and queued work');
      assert.equal(sourceFixture.lateProxyQueuedFetches, queuedFetches, 'queued stale route must not reach source');
      if (enable) assert.equal(proxyFixture.hits, oldHits, 'frozen direct work cannot select a newly enabled proxy');
      assert.equal(botApi.sendCalls, sends, 'policy-invalidated work cannot deliver media');
      await assertWorkspaceClean();
    }
    assert.equal(await invariant(), baseline);
    await fixtureSql("UPDATE dl_user_settings SET proxy_enabled=1,proxy_url='socks5://invalid-configuration',quality='corrupt-quality' WHERE user_id=42");
    await toggle(false);
    assert.equal(await fixtureSql('SELECT proxy_url,quality FROM dl_user_settings WHERE user_id=42'), 'socks5://invalid-configuration|corrupt-quality');
    const broken = await command();
    assert(!buttons(broken).some(b => b.callback_data?.startsWith('proxy:on:')));
    const corruptRow = await row();
    assert.equal((await click(broken, `proxy:on:${(await state()).revision}`)).ack.data.show_alert, true);
    assert.equal(await row(), corruptRow, 'invalid configuration cannot be enabled');
    assert.equal(botApi.error, undefined);
  } finally {
    sourceFixture.releaseGatedResponses();
    botApi.setExpectedUpload(null);
    await fixtureSql(`UPDATE users SET role=${literal(savedUser.role)},ban_until=${literal(savedUser.ban_until)} WHERE id=42`);
    if (saved) await fixtureSql(`UPDATE dl_user_settings SET ${Object.entries(saved).map(([name, value]) => `${name}=${literal(value)}`).join(',')} WHERE user_id=42`);
    else await fixtureSql('DELETE FROM dl_user_settings WHERE user_id=42');
    assert.equal(await row(), oldRow, 'proxy fixture restores the complete original settings row and revision');
  }
  return 'private proxy menu, stale/restarted callbacks, strict grammar and bans, EN/RU, real authenticated proxy/direct output, frozen-route invalidation in both directions, corrupt-config recovery and exact fixture restoration passed';
}
