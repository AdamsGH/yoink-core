import assert from 'node:assert/strict';
import { randomInt } from 'node:crypto';
import { commandHarness } from './command-harness.mjs';

export async function runLanguageScenarios(context) {
  const { botApi, fixtureSql, waitFor, restartApp, sourceFixture, sourcePort } = context;
  const saved = JSON.parse(await fixtureSql("SELECT json_object('language',language,'updated_at',updated_at,'ban_until',ban_until) FROM users WHERE id=42"));
  const userFields = (await fixtureSql("SELECT name FROM pragma_table_info('users') WHERE name NOT IN ('language','updated_at') ORDER BY cid")).trim().split('\n');
  const userOther = () => fixtureSql(`SELECT json_object(${userFields.map(field => `'${field}',${field}`).join(',')}) FROM users WHERE id=42`);
  const settings = () => fixtureSql('SELECT * FROM dl_user_settings WHERE user_id=42');
  const cookies = () => fixtureSql('SELECT * FROM cookies ORDER BY id');
  const jobs = () => fixtureSql('SELECT count(*) FROM runtime_operations');
  const grants = () => fixtureSql('SELECT * FROM user_permissions WHERE user_id=42 ORDER BY plugin,feature');
  const cache = () => fixtureSql('SELECT * FROM runtime_media_cache ORDER BY cache_key');
  const history = () => fixtureSql('SELECT * FROM runtime_delivery_units ORDER BY operation_id,attempt,ordinal');
  const before = { other: await userOther(), settings: await settings(), cookies: await cookies(), grants: await grants(), cache: await cache(), history: await history(), jobs: await jobs() };
  let next = 91000;
  const buttons = message => {
    const markup = message.data.reply_markup;
    return (typeof markup === 'string' ? JSON.parse(markup) : markup)?.inline_keyboard?.flat() ?? [];
  };
  const send = async (text, language = 'en', prefix = undefined) => {
    const start = botApi.calls.length;
    botApi.enqueue(42, 42, text, next++, language);
    return waitFor(() => botApi.calls.slice(start).find(c => c.method === 'sendmessage' && (!prefix || c.data.text.startsWith(prefix))), `language scenario ${text}`);
  };
  async function click(menu, data, options = {}) {
    const start = botApi.calls.length;
    const { expectEdit = false, ...callbackOptions } = options;
    const id = botApi.enqueueCallback(callbackOptions.actor ?? 42, 42, menu.messageId, data, callbackOptions);
    const ack = await waitFor(() => botApi.calls.slice(start).find(c => c.method === 'answercallbackquery' && c.data.callback_query_id === id), 'language callback acknowledgement');
    assert.equal(botApi.calls.slice(start).filter(c => c.method === 'answercallbackquery' && c.data.callback_query_id === id).length, 1);
    const findEdit = () => botApi.calls.slice(start).find(c => c.method === 'editmessagetext' && c.data.message_id === menu.messageId);
    const edit = expectEdit ? await waitFor(findEdit, 'language callback menu edit') : findEdit();
    return { ack, edit };
  }
  const state = async () => JSON.parse(await fixtureSql("SELECT json_object('language',language,'updated_at',updated_at) FROM users WHERE id=42"));
  try {
    await fixtureSql("UPDATE users SET language='en' WHERE id=42");
    const operations = await jobs();
    const menu = await send('/lang', 'ru');
    const choices = buttons(menu);
    assert.equal(choices.length, 2, 'bare /lang has exactly two choices');
    const current = await state();
    const en = choices.find(b => b.callback_data.startsWith('lang:en:'));
    const ru = choices.find(b => b.callback_data.startsWith('lang:ru:'));
    assert(en && ru);
    assert.deepEqual(new Set(choices.map(b => b.callback_data)), new Set([`lang:en:${current.updated_at}`, `lang:ru:${current.updated_at}`]));
    assert.match(menu.data.text, /English|Russian|language/i);
    assert.equal((en.text.includes('✓') || en.text.includes('✔')), true, 'current locale is visibly checked');
    assert.equal(ru.text.includes('✓') || ru.text.includes('✔'), false);
    const secondMenu = await send('/lang', 'ru');
    const secondRu = buttons(secondMenu).find(b => b.callback_data.startsWith('lang:ru:'));
    const selection = await click(menu, ru.callback_data, { expectEdit: true });
    assert(selection.edit, 'valid selection edits its menu');
    assert.equal(buttons({ data: { reply_markup: selection.edit.data.reply_markup } }).length, 0, 'success edit removes keyboard buttons');
    assert.match(selection.edit.data.text, /рус|язык/i);
    let chosen = await state();
    assert.equal(chosen.language, 'ru');
    assert(Number(chosen.updated_at) > Number(current.updated_at));
    const sameChoiceMenu = await send('/lang');
    const sameChoiceRu = buttons(sameChoiceMenu).find(b => b.callback_data.startsWith('lang:ru:'));
    const sameChoice = await click(sameChoiceMenu, sameChoiceRu.callback_data, { expectEdit: true });
    assert(sameChoice.edit, 'selecting the active language still succeeds');
    assert(Number((await state()).updated_at) > Number(chosen.updated_at), 'same-choice selection advances the revision');
    chosen = await state();
    const replay = await click(menu, ru.callback_data, { id: 'language-replay' });
    assert.equal(replay.edit, undefined);
    assert.equal((await state()).updated_at, chosen.updated_at);
    await restartApp();
    const abaMenu = await send('/lang');
    const abaEn = buttons(abaMenu).find(b => b.callback_data.startsWith('lang:en:'));
    const stale = await click(secondMenu, secondRu.callback_data);
    assert.equal(stale.edit, undefined);
    assert.equal((await state()).language, 'ru');
    const enChoice = await click(abaMenu, abaEn.callback_data, { expectEdit: true });
    assert(enChoice.edit);
    chosen = await state();
    assert.equal(chosen.language, 'en');
    assert(Number(chosen.updated_at) > Number(current.updated_at));
    const abaReplay = await click(secondMenu, secondRu.callback_data);
    assert.equal(abaReplay.edit, undefined);
    assert.equal((await state()).language, 'en', 'old choice cannot overwrite after switching back');
    for (const data of ['lang:fr:0', 'lang:en', 'lang:en:0:extra', 'lang:ru:-1', 'lang:ru:999999999999999999999999']) {
      const result = await click(abaMenu, data);
      assert.equal(result.edit, undefined);
      assert.equal((await state()).language, 'en');
    }
    const foreignStart = botApi.calls.length;
    botApi.enqueue(42, 42, '/lang@OTHER_BOT', next++, 'ru');
    await send('/help', 'en');
    assert.equal(botApi.calls.slice(foreignStart).filter(c => c.method === 'sendmessage').length, 1, 'foreign bot suffix is ignored');
    const ownSuffix = await send('/lang@YOINK_FIXTURE_BOT', 'ru');
    assert.equal(buttons(ownSuffix).length, 2, 'matching bot suffix is accepted');
    const usage = await send('/lang invalid', 'en');
    assert.match(usage.data.text, /Use \/lang without arguments/i);
    assert.equal(buttons(usage).length, 0);
    for (const options of [{ actor: 43 }, { isBot: true }, { inaccessible: true },
      { messagePatch: { sender_chat: { id: -10042, type: 'channel', title: 'Channel actor' } } },
      { messagePatch: { message_thread_id: 4 } },
      { messagePatch: { chat: { id: -42, type: 'group', title: 'Fixture' } } },
      { messagePatch: { from: { id: 900, is_bot: true, first_name: 'Foreign bot' } } }]) {
      const fresh = await send('/lang');
      const revision = (await state()).updated_at;
      const choice = buttons(fresh).find(b => b.callback_data.startsWith('lang:ru:'));
      const result = await click(fresh, choice.callback_data, options);
      assert.equal(result.edit, undefined);
      assert.equal((await state()).updated_at, revision, 'denial leaves the current revision unchanged');
      assert.equal((await state()).language, 'en');
      assert.equal(result.ack.data.text, undefined, 'foreign callbacks are acknowledged without revealing preferences');
    }
    for (const options of [{ messagePatch: { chat: { id: 43, type: 'private', first_name: 'Other chat' } } },
      { messagePatch: { from: { id: 123456, is_bot: false, first_name: 'Non-bot author' } } },
      { messagePatch: { from: undefined } }]) {
      const fresh = await send('/lang');
      const revision = (await state()).updated_at;
      const choice = buttons(fresh).find(b => b.callback_data.startsWith('lang:ru:'));
      const result = await click(fresh, choice.callback_data, options);
      assert.equal(result.edit, undefined);
      assert.equal((await state()).updated_at, revision);
      assert.equal((await state()).language, 'en');
      assert.equal(result.ack.data.text, undefined);
    }
    const bannedMenu = await send('/lang');
    await fixtureSql("UPDATE users SET ban_until=unixepoch('now')*1000000+600000000 WHERE id=42");
    const bannedRevision = (await state()).updated_at;
    const bannedChoice = buttons(bannedMenu).find(b => b.callback_data.startsWith('lang:ru:'));
    const banned = await click(bannedMenu, bannedChoice.callback_data);
    assert.equal(banned.edit, undefined);
    assert.equal(banned.ack.data.show_alert, true);
    assert.equal((await state()).updated_at, bannedRevision);
    assert.equal((await state()).language, 'en');
    await fixtureSql(`UPDATE users SET ban_until=${saved.ban_until === null ? 'NULL' : saved.ban_until} WHERE id=42`);
    await fixtureSql("UPDATE users SET language='ru' WHERE id=42");
    const persisted = await send('/help', 'en');
    assert.match(persisted.data.text, /команд|загруз/i, 'stored Russian overrides English Telegram metadata');
    await restartApp();
    const restarted = await send('/help', 'en');
    assert.match(restarted.data.text, /команд|загруз/i, 'language preference persists across restart');
    for (const [command, pattern] of [['/settings', /настройк/i], ['/proxy', /прокс/i], ['/cookie', /cookies|cookie|файлов cookie/i], ['/cut', /\/cut|фрагмент/i]]) {
      const reply = await send(command, 'en');
      assert.match(reply.data.text, pattern, `stored Russian localizes ${command}`);
    }
    for (const [command, data, messagePattern] of [
      ['/settings', 'prefs:invalid', /устарел|недейств/i],
      ['/proxy', 'proxy:invalid', /устарел|недейств/i],
      ['/cookie', 'cookie:invalid', /устарел|недейств/i],
      ['/cut', 'cut:invalid', /устарел|недейств/i],
    ]) {
      const menu = await send(command, 'en');
      const rejected = await click(menu, data);
      assert.equal(rejected.ack.data.show_alert, true, `${command} invalid callback must be an alert`);
      assert.match(rejected.ack.data.text, messagePattern, `stored Russian localizes ${command} callback error`);
    }
    const badCookieCaptionStart = botApi.calls.length;
    botApi.enqueueDocument(42, 42, '/cookie invalid domain!', next++, 'language-invalid-caption', { filename: 'invalid.txt', language: 'en' });
    const badCaption = await waitFor(() => botApi.calls.slice(badCookieCaptionStart).find(c => c.method === 'sendmessage' && c.data.chat_id === 42), 'localized invalid cookie caption response');
    assert.match(badCaption.data.text, /файл Netscape|подпись/i, 'stored Russian localizes invalid cookie-upload caption guidance');
    await fixtureSql("UPDATE users SET language='en' WHERE id=42");
    const help = await send('/help', 'ru');
    assert.match(help.data.text, /commands|download/i, 'stored English overrides Russian Telegram metadata');
    await fixtureSql("UPDATE users SET language='xx' WHERE id=42");
    const fallback = await send('/help', 'ru');
    assert.match(fallback.data.text, /команд|загруз/i, 'unsupported stored locale falls back to Telegram metadata');
    assert.equal(await fixtureSql("SELECT language FROM users WHERE id=42"), 'xx', 'fallback does not silently rewrite unsupported stored language');
    await fixtureSql("UPDATE users SET language='en' WHERE id=42");
    assert.equal(await userOther(), before.other);
    assert.equal(await settings(), before.settings);
    assert.equal(await cookies(), before.cookies);
    assert.equal(await grants(), before.grants);
    assert.equal(await cache(), before.cache);
    assert.equal(await history(), before.history);
    assert.equal(await jobs(), operations, 'language menus and callbacks do not create jobs');
    assert.equal(await fixtureSql("SELECT language FROM users WHERE id=42"), 'en');

  } finally {
    const literal = value => value === null ? 'NULL' : typeof value === 'number' ? String(value) : `'${String(value).replaceAll("'", "''")}'`;
    await fixtureSql(`UPDATE users SET language=${literal(saved.language)},updated_at=${literal(saved.updated_at)},ban_until=${literal(saved.ban_until)} WHERE id=42`);
  }
  assert.equal(await userOther(), before.other);
  assert.equal(await settings(), before.settings);
  assert.equal(await cookies(), before.cookies);
  assert.equal(await grants(), before.grants);
  assert.equal(await cache(), before.cache);
  assert.equal(await history(), before.history);
  assert.equal(await jobs(), before.jobs);

  await runOrdinaryLanguageScenarios(context);

  // This is a separate real-worker admission phase. Its progress locale must be
  // frozen at admission even if the stored preference changes while fetch is gated.
  const admissionBaseline = { settings: await settings(), cookies: await cookies(), grants: await grants() };
  const h = commandHarness(context, 1_250_000);
  const checkpoint = id => fixtureSql(`SELECT finished || '|' || COALESCE(last_text,'') FROM runtime_progress_notices WHERE operation_id='${id}'`);
  await fixtureSql("UPDATE users SET language='ru' WHERE id=42");
  sourceFixture.closeProgressGate();
  let admitted;
  try {
    admitted = await h.submit(`/video http://127.0.0.1:${sourcePort}/progress.mp4?case=language-admission`, { language: 'en' });
    await waitFor(async () => /Загрузка\./.test(await checkpoint(admitted.operationId)), 'Russian progress from stored admission language');
    const changeMenu = await send('/lang', 'ru');
    await click(changeMenu, buttons(changeMenu).find(b => b.callback_data.startsWith('lang:en:')).callback_data, { expectEdit: true });
    assert.match(await checkpoint(admitted.operationId), /Загрузка\./, 'active progress retains its admission language after preference change');
    assert.equal(await settings(), admissionBaseline.settings);
    assert.equal(await cookies(), admissionBaseline.cookies);
    assert.equal(await grants(), admissionBaseline.grants);
    sourceFixture.releaseProgressResponses();
    await h.succeeded(admitted.operationId);
    await waitFor(async () => (await checkpoint(admitted.operationId)).startsWith('1|'), 'language admission terminal progress');
    assert.match(await checkpoint(admitted.operationId), /^1\|Готово\. Доставлено файлов: 1\/1\./);
    const english = await h.submit(`/video http://127.0.0.1:${sourcePort}/media?case=language-new-english`, { language: 'ru' });
    await h.succeeded(english.operationId);
    await waitFor(async () => (await checkpoint(english.operationId)).startsWith('1|'), 'new English admission terminal progress');
    assert.match(await checkpoint(english.operationId), /^1\|Done\. Delivered files: 1\/1\./);
  } finally {
    sourceFixture.releaseProgressResponses();
    if (admitted) await waitFor(async () => (await context.queryJob(admitted.operationId)).startsWith('succeeded|') || (await context.queryJob(admitted.operationId)).startsWith('failed|'), 'language admission worker cleanup');
    await fixtureSql(`UPDATE users SET language='${String(saved.language).replaceAll("'", "''")}',updated_at=${saved.updated_at} WHERE id=42`);
  }
  return 'authorized private /lang menu, ordinary-user isolation/revocation, revision-fenced selection/replay/restart, locale precedence, language-only state invariants and gated worker admission-language snapshot';
}

async function runOrdinaryLanguageScenarios({ botApi, fixtureSql, waitFor, restartApp }) {
  const actor = 6_100_000_000_000 + randomInt(1_000_000_000);
  let messageId = 94_000;
  const owner = await fixtureSql("SELECT language||'|'||updated_at FROM users WHERE id=42");
  const operations = await fixtureSql('SELECT count(*) FROM runtime_operations');
  const state = () => fixtureSql(`SELECT language||'|'||updated_at FROM users WHERE id=${actor}`);
  const buttons = call => {
    const markup = call.data.reply_markup;
    return (typeof markup === 'string' ? JSON.parse(markup) : markup)?.inline_keyboard?.flat() ?? [];
  };
  async function menu() {
    const start = botApi.calls.length;
    botApi.enqueue(actor, actor, '/lang', messageId++, 'en');
    return waitFor(() => botApi.calls.slice(start).find(call => call.method === 'sendmessage' && call.data.chat_id === actor && buttons(call).length === 2), 'ordinary private language menu');
  }
  async function click(message, data, from = actor) {
    const start = botApi.calls.length;
    const id = botApi.enqueueCallback(from, actor, message.messageId, data);
    const ack = await waitFor(() => botApi.calls.slice(start).find(call => call.method === 'answercallbackquery' && call.data.callback_query_id === id), 'ordinary language callback acknowledgement');
    return { ack, edit: botApi.calls.slice(start).find(call => call.method === 'editmessagetext' && call.data.message_id === message.messageId && call.data.chat_id === actor) };
  }
  await fixtureSql(`INSERT INTO users(id,role,language,created_at,updated_at) VALUES(${actor},'user','en',1,1)`);
  try {
    const initial = await menu();
    const ru = buttons(initial).find(button => button.callback_data.startsWith('lang:ru:'));
    assert(ru);
    const before = await state();
    const selected = await click(initial, ru.callback_data);
    assert(selected.edit, 'ordinary user can change their own language');
    assert.match(selected.edit.data.text, /рус|язык/i);
    assert.match(await state(), /^ru\|/);
    assert.notEqual(await state(), before);
    const persisted = await state();
    await restartApp();
    const stale = await click(initial, ru.callback_data);
    assert.equal(stale.edit, undefined);
    assert.equal(await state(), persisted, 'restart retains language and fences old selection');
    const current = await menu();
    assert.match(current.data.text, /язык/i, 'saved locale overrides client locale for ordinary users');
    const en = buttons(current).find(button => button.callback_data.startsWith('lang:en:'));
    const foreign = await click(current, en.callback_data, 42);
    assert.equal(foreign.edit, undefined);
    assert.equal(foreign.ack.data.text, undefined);
    assert.equal(await state(), persisted, 'owner cannot mutate another private chat by callback');
    for (const mutation of ["ban_until=9223372036854775807", "role='restricted'", "role='banned'"]) {
      await fixtureSql(`UPDATE users SET role='user',ban_until=NULL WHERE id=${actor}`);
      const fresh = await menu();
      const choice = buttons(fresh).find(button => button.callback_data.startsWith('lang:en:'));
      await fixtureSql(`UPDATE users SET ${mutation} WHERE id=${actor}`);
      const denied = await click(fresh, choice.callback_data);
      assert.equal(denied.edit, undefined);
      assert.equal(denied.ack.data.show_alert, true);
      assert.equal(await state(), persisted, 'authorization is checked again at selection');
    }
    assert.equal(await fixtureSql("SELECT language||'|'||updated_at FROM users WHERE id=42"), owner);
    assert.equal(await fixtureSql('SELECT count(*) FROM runtime_operations'), operations);
  } finally {
    await fixtureSql(`DELETE FROM users WHERE id=${actor}`);
  }
}
