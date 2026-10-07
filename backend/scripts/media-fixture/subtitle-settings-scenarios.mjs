import assert from 'node:assert/strict';
import { commandHarness } from './command-harness.mjs';
import { subtitleBytes } from './subtitle-source.mjs';

export async function runSubtitleSettingsScenarios(context) {
  const { botApi, fixtureSql, sourcePort, sourceFixture, proxyUrl, waitFor, restartApp, assertWorkspaceClean } = context;
  const h = commandHarness(context, 1_310_000);
  const settingsColumns = (await fixtureSql("SELECT name FROM pragma_table_info('dl_user_settings') ORDER BY cid")).trim().split('\n');
  const userColumns = (await fixtureSql("SELECT name FROM pragma_table_info('users') ORDER BY cid")).trim().split('\n');
  const literal = value => value === null ? 'NULL' : typeof value === 'number' ? String(value) : `'${String(value).replaceAll("'", "''")}'`;
  const jsonRow = (table, columns, where) => fixtureSql(`SELECT json_object(${columns.map(name => `'${name}',${name}`).join(',')}) FROM ${table} ${where}`);
  const settingRow = () => jsonRow('dl_user_settings', settingsColumns, 'WHERE user_id=42');
  const userRow = () => jsonRow('users', userColumns, 'WHERE id=42');
  const savedSettings = await settingRow();
  const savedUser = await userRow();
  const savedCookieRows = await fixtureSql('SELECT * FROM cookies ORDER BY id');
  const savedGrants = await fixtureSql('SELECT * FROM user_permissions WHERE user_id=42 ORDER BY plugin,feature');
  const savedOtherSettings = await fixtureSql('SELECT * FROM dl_user_settings WHERE user_id<>42 ORDER BY user_id');
  const savedOtherUsers = await fixtureSql('SELECT * FROM users WHERE id<>42 ORDER BY id');
  const savedJobs = await fixtureSql('SELECT id FROM runtime_operations ORDER BY id');
  const fields = async () => {
    const row = await settingRow();
    return row.trim() ? JSON.parse(row) : null;
  };
  const state = async () => {
    const row = await fields();
    return { revision: row?.updated_at ?? 0, language: row?.subs_lang ?? 'en', automatic: row?.subs_auto ?? 0 };
  };
  let next = 1_311_000;
  const buttons = message => message.data.reply_markup?.inline_keyboard?.flat() ?? [];
  async function send(text, language = 'en', prefix) {
    const start = botApi.calls.length;
    botApi.enqueue(42, 42, text, next++, language);
    return waitFor(() => botApi.calls.slice(start).find(call => call.method === 'sendmessage' && Number(call.data.chat_id) === 42 && (!prefix || call.data.text.startsWith(prefix))), `subtitle settings response ${text}`);
  }
  async function click(menu, data, options = {}, expectEdit = false) {
    const start = botApi.calls.length;
    const id = botApi.enqueueCallback(options.actor ?? 42, 42, menu.messageId, data, options);
    const ack = await waitFor(() => botApi.calls.slice(start).find(call => call.method === 'answercallbackquery' && call.data.callback_query_id === id), 'subtitle settings callback acknowledgement');
    const edit = expectEdit ? await waitFor(() => botApi.calls.slice(start).find(call => call.method === 'editmessagetext' && call.data.message_id === menu.messageId), 'subtitle settings menu refresh') : undefined;
    if (!expectEdit) assert(!botApi.calls.slice(start).some(call => call.method === 'editmessagetext' && call.data.message_id === menu.messageId), 'denied/stale callback cannot edit its menu');
    assert.equal(botApi.calls.slice(start).filter(call => call.method === 'answercallbackquery' && call.data.callback_query_id === id).length, 1);
    return { ack, edit };
  }
  async function open(language = 'en', suffix = '') {
    const menu = await send(`/subs${suffix}`, language);
    assert.equal(Number(menu.data.chat_id), 42, 'standalone subtitle settings are delivered to the private owner chat');
    assert(buttons(menu).length > 0, 'subtitle settings menu serves inline buttons');
    assert(buttons(menu).every(button => /^subs:(lang|auto|enabled|ask):/.test(button.callback_data)), 'standalone menu exposes only subtitle settings controls');
    return menu;
  }
  async function servedChoice(menu, prefix) {
    const button = buttons(menu).find(value => value.callback_data.startsWith(prefix));
    assert(button, `expected served button ${prefix}`);
    return button.callback_data;
  }
  const noMenuJobs = async before => assert.equal(await fixtureSql('SELECT id FROM runtime_operations ORDER BY id'), before, 'menu actions cannot create or replace jobs');
  const userImmutableColumns = userColumns.filter(name => !['language', 'updated_at', 'ban_until'].includes(name));
  const immutableState = async () => ({
    user: await jsonRow('users', userImmutableColumns, 'WHERE id=42'),
    cookies: await fixtureSql('SELECT * FROM cookies ORDER BY id'),
    grants: await fixtureSql('SELECT * FROM user_permissions WHERE user_id=42 ORDER BY plugin,feature'),
    otherSettings: await fixtureSql('SELECT * FROM dl_user_settings WHERE user_id<>42 ORDER BY user_id'),
    otherUsers: await fixtureSql('SELECT * FROM users WHERE id<>42 ORDER BY id'),
  });
  const immutableBefore = await immutableState();
  const assertImmutable = async () => assert.deepEqual(await immutableState(), immutableBefore, 'settings interactions preserve user identity, grants, cookies, and other fixture rows');
  try {
    const initialJobs = savedJobs;
    await fixtureSql('DELETE FROM dl_user_settings WHERE user_id=42');
    const startCalls = botApi.calls.length;
    let menu = await open('en');
    assert.match(menu.data.text, /subtitle|subtitles/i, 'English metadata opens English standalone subtitle settings');
    assert.equal(await settingRow(), '', 'opening /subs does not create a missing settings row');
    const available = buttons(menu).map(button => button.callback_data);
    const supported = ['en', 'ru', 'zh', 'ja', 'ar', 'hi', 'de', 'fr', 'es', 'pt', 'zh-Hans'];
    for (const code of supported) assert(available.includes(`subs:lang:${code}:0`), `served language option ${code} has revision zero`);
    for (const action of ['auto', 'enabled', 'ask']) {
      assert.equal(available.filter(data => data === `subs:${action}:true:0`).length, 1, `menu serves the opposite ${action} toggle for the false default at revision zero`);
    }
    assert(buttons(menu).some(button => /^en(?: ✓)?$/.test(button.text)) && buttons(menu).some(button => /^ru(?: ✓)?$/.test(button.text)), 'language menu labels expose language codes');
    await noMenuJobs(initialJobs);

    // A first served choice creates defaults, while the missing row was observable as revision zero.
    const firstChoice = await click(menu, await servedChoice(menu, 'subs:lang:ru:0'), {}, true);
    assert(firstChoice.edit);
    let afterSelect = await fields();
    assert(afterSelect, 'first served mutation creates the owner settings defaults');
    assert.equal(afterSelect.subs_lang, 'ru');
    assert(Number(afterSelect.updated_at) > 0);
    await assertImmutable();

    await fixtureSql("UPDATE users SET language='ru' WHERE id=42");
    const russian = await open('en', '@YOINK_FIXTURE_BOT');
    assert.match(russian.data.text, /субтитр/i, 'saved user language overrides English Telegram metadata');
    await fixtureSql("UPDATE users SET language='en' WHERE id=42");
    const english = await open('ru');
    assert.match(english.data.text, /subtitle|subtitles/i, 'saved English user language overrides Russian Telegram metadata');
    await noMenuJobs(initialJobs);

    // Select preferences only from served controls and confirm writes are limited to those fields plus revision.
    const beforeSelect = await fields();
    const ruButton = await servedChoice(english, `subs:lang:ru:${beforeSelect.updated_at}`);
    const selected = await click(english, ruButton, {}, true);
    assert(selected.edit, 'successful language selection re-renders the menu');
    afterSelect = await fields();
    assert.equal(afterSelect.subs_lang, 'ru');
    assert(Number(afterSelect.updated_at) > Number(beforeSelect.updated_at));
    for (const key of settingsColumns) if (!['subs_lang', 'updated_at'].includes(key)) assert.deepEqual(afterSelect[key], beforeSelect[key], `${key} remains unchanged by language choice`);
    assert(buttons({ data: selected.edit.data }).some(button => button.callback_data.endsWith(`:${afterSelect.updated_at}`)), 'refreshed buttons carry the new revision');
    menu = await open();
    const targetAutomatic = afterSelect.subs_auto ? 'false' : 'true';
    const autoButton = await servedChoice(menu, `subs:auto:${targetAutomatic}:${afterSelect.updated_at}`);
    const beforeAuto = afterSelect;
    await click(menu, autoButton, {}, true);
    afterSelect = await fields();
    assert.equal(afterSelect.subs_auto, targetAutomatic === 'true' ? 1 : 0);
    assert(Number(afterSelect.updated_at) > Number(beforeAuto.updated_at));
    for (const key of settingsColumns) if (!['subs_auto', 'updated_at'].includes(key)) assert.deepEqual(afterSelect[key], beforeAuto[key], `${key} remains unchanged by automatic choice`);
    const sameChoiceMenu = await open();
    const sameBefore = await fields();
    await click(sameChoiceMenu, await servedChoice(sameChoiceMenu, `subs:lang:${sameBefore.subs_lang}:${sameBefore.updated_at}`), {}, true);
    const sameAfter = await fields();
    assert(Number(sameAfter.updated_at) > Number(sameBefore.updated_at), 'same-choice language selection advances revision');
    afterSelect = sameAfter;
    await assertImmutable();

    // The shared revision fences callbacks across every settings editor.
    menu = await open();
    const staleLanguage = await servedChoice(menu, `subs:lang:en:${afterSelect.updated_at}`);
    await send('/settings quality best');
    const afterSettings = await settingRow();
    const staleAcrossSettings = await click(menu, staleLanguage);
    assert.equal(staleAcrossSettings.ack.data.show_alert, true);
    assert.equal(await settingRow(), afterSettings, '/settings mutation invalidates the old /subs callback');

    assert(proxyUrl, 'media fixture supplies a proxy URL for shared-revision callback coverage');
    await fixtureSql(`UPDATE dl_user_settings SET proxy_url=${literal(proxyUrl)},proxy_enabled=0 WHERE user_id=42`);
    const proxyMenu = await send('/proxy');
    const proxyButton = buttons(proxyMenu).find(button => /^proxy:on:/.test(button.callback_data));
    assert(proxyButton, 'configured proxy menu serves an enable button');
    const freshSubtitleMenu = await open();
    const staleFromProxyMenu = await servedChoice(freshSubtitleMenu, `subs:lang:en:${(await state()).revision}`);
    await click(proxyMenu, proxyButton.callback_data, {}, true);
    const afterProxy = await settingRow();
    const crossProxy = await click(freshSubtitleMenu, staleFromProxyMenu);
    assert.equal(crossProxy.ack.data.show_alert, true);
    assert.equal(await settingRow(), afterProxy, '/proxy mutation invalidates the old /subs callback');
    const settingsMenuBeforeSubs = await send('/settings');
    const settingsField = await click(settingsMenuBeforeSubs, await servedChoice(settingsMenuBeforeSubs, 'prefs:quality:_menu'), {}, true);
    settingsMenuBeforeSubs.data = settingsField.edit.data;
    const proxyMenuBeforeSubs = await send('/proxy');
    const oldSettingsButton = buttons(settingsMenuBeforeSubs).find(button => /^prefs:.*:\d+$/.test(button.callback_data))?.callback_data;
    const oldProxyButton = buttons(proxyMenuBeforeSubs).find(button => /^proxy:off:/.test(button.callback_data))?.callback_data;
    assert(oldSettingsButton && oldProxyButton, 'settings and proxy controls are served before reverse invalidation probe');
    const subsMutationMenu = await open();
    await click(subsMutationMenu, await servedChoice(subsMutationMenu, `subs:lang:de:${(await state()).revision}`), {}, true);
    assert.equal((await click(settingsMenuBeforeSubs, oldSettingsButton)).ack.data.show_alert, true, '/subs mutation invalidates old /settings buttons');
    assert.equal((await click(proxyMenuBeforeSubs, oldProxyButton)).ack.data.show_alert, true, '/subs mutation invalidates old /proxy buttons');
    const disableProxyMenu = await send('/proxy');
    const disableProxy = buttons(disableProxyMenu).find(button => /^proxy:off:/.test(button.callback_data));
    assert(disableProxy, 'fresh /proxy menu serves disable after cross-proxy mutation');
    await click(disableProxyMenu, disableProxy.callback_data, {}, true);

    // Malformed and legacy embedded-toggle callback data must be rejected without changing the current row.
    const malformed = await open();
    const malformedRow = await settingRow();
    for (const data of ['subs:lang:xx:0', 'subs:lang:en', 'subs:lang:en:0:extra', 'subs:auto:maybe:0', 'subs:auto:true:-1', 'subs:toggle:true:0', 'subs:auto:on']) {
      assert.equal((await click(malformed, data)).ack.data.show_alert, true, `reject malformed/legacy data ${data}`);
      assert.equal(await settingRow(), malformedRow);
    }
    assert.equal((await click(malformed, 'prefs:subs_enabled:true:0')).ack.data.show_alert, true, 'a valid cross-editor callback with stale revision must be rejected');
    assert.equal(await settingRow(), malformedRow);
    // Each authorization probe starts with a current callback revision so a stale guard cannot mask access control.
    for (const options of [
      { actor: 43 }, { isBot: true }, { inaccessible: true },
      { messagePatch: { chat: { id: -42, type: 'group', title: 'Fixture group' } } },
      { messagePatch: { message_thread_id: 17 } },
      { messagePatch: { from: { id: 999, is_bot: true, first_name: 'Foreign bot' } } },
      { messagePatch: { chat: { id: 43, type: 'private', first_name: 'Foreign owner' } } },
    ]) {
      const probe = await open();
      const revision = (await state()).revision;
      const row = await settingRow();
      const data = await servedChoice(probe, `subs:lang:en:${revision}`);
      const denied = await click(probe, data, options);
      assert.equal(denied.edit, undefined, 'unauthorized callback cannot edit or re-render the menu');
      assert.equal(await settingRow(), row, 'denial preserves complete settings row and revision');
    }
    const bannedMenu = await open();
    const bannedData = await servedChoice(bannedMenu, `subs:lang:en:${(await state()).revision}`);
    await fixtureSql("UPDATE users SET ban_until=unixepoch('now')*1000000+600000000 WHERE id=42");
    const banned = await click(bannedMenu, bannedData);
    assert.equal(banned.ack.data.show_alert, true);
    const bannedCommand = await send('/subs');
    assert.equal(buttons(bannedCommand).length, 0, 'banned owner cannot open the subtitle settings menu');
    assert.match(bannedCommand.data.text, /not allowed|запрещ/i, 'banned command receives denial');
    await fixtureSql(`UPDATE users SET ban_until=${literal(JSON.parse(savedUser).ban_until)} WHERE id=42`);
    assert.equal(await settingRow(), malformedRow, 'ban denial preserves settings state');
    await noMenuJobs(initialJobs);

    // Real subtitle commands consume settings changed through the native menu, without video transfer.
    menu = await open();
    if ((await state()).automatic) {
      const manual = await servedChoice(menu, `subs:auto:false:${(await state()).revision}`);
      await click(menu, manual, {}, true);
    }
    assert.equal((await state()).automatic, 0);
    let preferences = await fields();
    const langMenu = await open();
    await click(langMenu, await servedChoice(langMenu, `subs:lang:ru:${preferences.updated_at}`), {}, true);
    preferences = await fields();
    await assertImmutable();
    const url = `http://127.0.0.1:${sourcePort}/subtitles.html?case=native-settings`;
    botApi.setExpectedUpload({ method: 'senddocument', filename: 'subtitles-ru.vtt', mimeType: 'text/vtt', preserveMdat: false });
    const videoRequests = sourceFixture.subtitleVideoRequests;
    const outputs = botApi.uploadCalls;
    const command = `/subs ${url}`;
    const operation = await h.submit(command);
    await h.succeeded(operation.operationId);
    assert.equal(botApi.uploadCalls, outputs + 1);
    assert.deepEqual(botApi.lastUploadedBytes, subtitleBytes('ru'));
    assert.deepEqual((await h.request(operation.operationId)).params.subtitles, { language: 'ru', automatic: false });
    botApi.setExpectedUpload({ method: 'senddocument', filename: 'subtitles-en.vtt', mimeType: 'text/vtt', preserveMdat: false });
    const override = await h.submit(`${command} --subtitle-language en --subtitle-auto true`);
    await h.succeeded(override.operationId);
    assert.deepEqual((await h.request(override.operationId)).params.subtitles, { language: 'en', automatic: true });
    const afterOverride = await fields();
    assert.equal(afterOverride.subs_lang, preferences.subs_lang);
    assert.equal(afterOverride.subs_auto, preferences.subs_auto, 'explicit overrides preserve saved choices');
    assert.equal(sourceFixture.subtitleVideoRequests, videoRequests, 'subtitle processing and menu actions fetch zero video bytes');
    const mediaJobs = await fixtureSql('SELECT id FROM runtime_operations ORDER BY id');
    assert.notEqual(mediaJobs, initialJobs, 'real subtitle commands create worker operations');
    await assertWorkspaceClean();

    const replayMenu = await open();
    const oldRevision = Number((await state()).revision);
    const oldButton = await servedChoice(replayMenu, `subs:lang:en:${oldRevision}`);
    const freshButton = await servedChoice(await open(), `subs:lang:ru:${oldRevision}`);
    await click(replayMenu, oldButton, {}, true);
    const changedRow = await settingRow();
    await restartApp();
    const restartState = await fields();
    assert.equal(restartState.subs_lang, 'en');
    const replayed = await click(replayMenu, oldButton);
    assert.equal(replayed.ack.data.show_alert, true);
    assert.equal(await settingRow(), changedRow, 'stale replay after restart cannot reapply a choice');
    const staleAfterRestart = await click(replayMenu, freshButton);
    assert.equal(staleAfterRestart.ack.data.show_alert, true);
    assert.equal(await fixtureSql('SELECT id FROM runtime_operations ORDER BY id'), mediaJobs, 'restart and stale callback probes do not create jobs');
    const foreignStart = botApi.calls.length;
    botApi.enqueue(42, 42, '/subs@OTHER_BOT', next++, 'en');
    await send('/help');
    assert.equal(botApi.calls.slice(foreignStart).filter(call => call.method === 'sendmessage').length, 1, 'foreign bare-command suffix is silent');
    await assertImmutable();
    assert.equal(botApi.calls.slice(startCalls).filter(call => call.method === 'sendmessage' && Number(call.data.chat_id) !== 42).length, 0, 'settings responses never leave the owner private chat');
  } finally {
    botApi.setExpectedUpload(null);
    const restoreRow = async (table, key, keyValue, rowText, columns) => {
      if (!rowText.trim()) {
        await fixtureSql(`DELETE FROM ${table} WHERE ${key}=${literal(keyValue)}`);
        return;
      }
      const row = JSON.parse(rowText);
      const values = columns.map(name => `${name}=${literal(row[name])}`).join(',');
      await fixtureSql(`INSERT INTO ${table} (${columns.join(',')}) VALUES (${columns.map(name => literal(row[name])).join(',')}) ON CONFLICT(${key}) DO UPDATE SET ${values}`);
    };
    await restoreRow('dl_user_settings', 'user_id', 42, savedSettings, settingsColumns);
    await restoreRow('users', 'id', 42, savedUser, userColumns);
    assert.equal(await settingRow(), savedSettings, 'cleanup restores every settings field including revision');
    assert.equal(await userRow(), savedUser, 'cleanup restores user language, updated_at, ban, and all other fields');
    assert.equal(await fixtureSql('SELECT * FROM cookies ORDER BY id'), savedCookieRows);
    assert.equal(await fixtureSql('SELECT * FROM user_permissions WHERE user_id=42 ORDER BY plugin,feature'), savedGrants);
    assert.equal(await fixtureSql('SELECT * FROM dl_user_settings WHERE user_id<>42 ORDER BY user_id'), savedOtherSettings);
    assert.equal(await fixtureSql('SELECT * FROM users WHERE id<>42 ORDER BY id'), savedOtherUsers);
  }
  return 'bare and suffixed /subs settings menu, shared revision fencing and authorization probes, preference-only persistence, real subtitle output and override behavior, restart/replay, and exact fixture-state restoration';
}
