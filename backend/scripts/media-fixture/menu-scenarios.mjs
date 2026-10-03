import assert from 'node:assert/strict';
import { createHmac, randomInt } from 'node:crypto';

const ordinary = ['audio', 'cancel', 'download', 'help', 'image', 'job', 'start', 'video'];
const ownerOnly = ['cookie', 'cut', 'format', 'lang', 'list', 'playlist', 'proxy', 'quality', 'settings', 'status', 'subs', 'uncache'];
const unavailable = ['about', 'ban_time', 'block', 'broadcast', 'group', 'runtime', 'stats', 'summary', 'thread', 'tldr', 'unblock'];
const sortedNames = commands => commands.map(({ command }) => command).sort();

export async function runMenuScenarios(context) {
  const { botApi, fixtureSql, waitFor, restartApp, appHttpPort, telegramToken } = context;
  const actor = 6_000_000_000_000 + randomInt(1_000_000_000);
  const scope = { type: 'chat', chat_id: actor };
  const savedPolicy = await fixtureSql("SELECT value FROM bot_settings WHERE key='bot_access_mode'");
  const literal = value => `'${String(value).replaceAll("'", "''")}'`;
  const base = `http://127.0.0.1:${appHttpPort}`;
  let release;
  async function request(path, { token, method = 'GET', body, expected = 200 } = {}) {
    const response = await fetch(base + path, {
      method, signal: AbortSignal.timeout(6000),
      headers: { ...(token ? { authorization: `Bearer ${token}` } : {}), ...(body === undefined ? {} : { 'content-type': 'application/json' }) },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    assert.equal(response.status, expected, `${method} ${path}`);
    const text = await response.text();
    return text ? JSON.parse(text) : null;
  }
  async function login(id) {
    const auth_date = String(Math.floor(Date.now() / 1000));
    const user = JSON.stringify({ id, first_name: 'Menu Fixture', username: `menus_${id}` });
    const secret = createHmac('sha256', 'WebAppData').update(telegramToken).digest();
    const hash = createHmac('sha256', secret).update(`auth_date=${auth_date}\nuser=${user}`).digest('hex');
    return (await request('/api/v1/auth/token', { method: 'POST', body: { init_data: new URLSearchParams({ auth_date, user, hash }).toString() } })).access_token;
  }
  const matches = (commands, expected) => JSON.stringify(sortedNames(commands)) === JSON.stringify([...expected].sort());
  async function menu(id, expected, language, label) {
    return waitFor(() => {
      const menus = ['en', 'ru', ''].map(locale => botApi.commandMenus.privateCommands(id, locale));
      if (!menus.every(commands => matches(commands, expected))) return false;
      if (language === 'ru' && !menus.every(commands => commands.every(({ description }) => /\p{Script=Cyrillic}/u.test(description)))) return false;
      if (language === 'en' && !menus.every(commands => commands.every(({ description }) => !/\p{Script=Cyrillic}/u.test(description)))) return false;
      return menus[0];
    }, label, 18000);
  }
  try {
    const ownerToken = await login(42);
    await menu(42, [...ordinary, ...ownerOnly], 'en', 'configured-owner implemented command menu');
    for (const type of ['default', 'all_private_chats', 'all_group_chats', 'all_chat_administrators']) {
      for (const language_code of ['', 'en', 'ru']) {
        const commands = sortedNames(botApi.commandMenus.get({ scope: { type }, language_code }));
        assert(!commands.some(command => ownerOnly.includes(command) || unavailable.includes(command)), `${type}/${language_code} cannot expose private controls or unimplemented commands`);
      }
    }
    await request('/api/v1/bot-settings', { token: ownerToken, method: 'PATCH', body: { bot_access_mode: 'approved_only' } });
    const actorToken = await login(actor);
    await waitFor(() => botApi.calls.some(call => ['setmycommands', 'deletemycommands'].includes(call.method) && call.data.scope?.chat_id === actor), 'restricted registration refresh without Telegram traffic');
    assert(!sortedNames(botApi.commandMenus.privateCommands(actor, 'en')).includes('video'), 'pending users cannot inherit download commands');
    const patch = body => request(`/api/v1/users/${actor}`, { token: ownerToken, method: 'PATCH', body });
    await patch({ role: 'user' });
    await menu(actor, ordinary, 'en', 'HTTP approval updates private command menu');
    await patch({ role: 'admin' });
    await menu(actor, ordinary, 'en', 'database admin cannot receive configured-owner controls');
    await patch({ language: 'ru' });
    await menu(actor, ordinary, 'ru', 'HTTP admin language refresh clears English client override');
    await request('/api/v1/settings', { token: actorToken, method: 'PATCH', body: { language: 'en' } });
    await menu(actor, ordinary, 'en', 'own settings language refresh clears Russian client override');
    const beforeDenied = await fixtureSql(`SELECT role||'|'||language||'|'||updated_at FROM users WHERE id=${actor}`);
    await request(`/api/v1/users/${actor}`, { token: ownerToken, method: 'PATCH', body: { language: 'xx' }, expected: 422 });
    assert.equal(await fixtureSql(`SELECT role||'|'||language||'|'||updated_at FROM users WHERE id=${actor}`), beforeDenied);
    await menu(actor, ordinary, 'en', 'rejected mutation preserves command menu');
    await request(`/api/v1/users/${actor}/permissions`, { token: ownerToken, method: 'POST', body: { plugin: 'dl', feature: 'inline' } });
    await menu(actor, ordinary, 'en', 'grant cannot advertise an unimplemented inline command');
    await request(`/api/v1/users/${actor}/permissions/dl/inline`, { token: ownerToken, method: 'DELETE' });
    await menu(actor, ordinary, 'en', 'grant revocation preserves implemented role-gated catalog');
    await patch({ ban_until: new Date(Date.now() + 600_000).toISOString() });
    await waitFor(() => ['en', 'ru', ''].every(language => !sortedNames(botApi.commandMenus.privateCommands(actor, language)).includes('video')), 'ban removes download menu without Telegram traffic');
    await patch({ ban_until: null });
    await menu(actor, ordinary, 'en', 'unban restores role-gated commands');

    for (const mode of ['rate-limit-once', 'api-error-once', 'socket-drop-once']) {
      const start = botApi.calls.length;
      botApi.setMenuMode(mode, actor);
      await patch({ language: 'ru' });
      await menu(actor, ordinary, 'ru', `${mode} menu reconciliation recovery`);
      const calls = botApi.calls.slice(start).filter(call => call.data?.scope?.chat_id === actor);
      const failed = calls.findIndex(call => call.status === mode);
      assert(failed >= 0, `${mode} fault must exercise the real Bot API transport`);
      if (mode === 'rate-limit-once') {
        const next = calls.slice(failed + 1).find(call => ['setmycommands', 'deletemycommands'].includes(call.method));
        assert(next && next.receivedAt - calls[failed].receivedAt >= 1850, 'menu retries must honor the two-second Bot API retry_after');
      }
      await patch({ language: 'en' });
      await menu(actor, ordinary, 'en', `${mode} subsequent mutation is not lost`);
    }

    release = botApi.holdNextMenu(actor);
    const start = botApi.calls.length;
    await patch({ language: 'ru' });
    await waitFor(() => botApi.calls.slice(start).some(call => call.status === 'hold-once' && call.data.scope?.chat_id === actor), 'first concurrent refresh enters Bot API');
    await patch({ role: 'restricted', language: 'en' });
    assert.equal(await fixtureSql(`SELECT role||'|'||language FROM users WHERE id=${actor}`), 'restricted|en', 'mutation commits while an earlier menu request is held');
    release(); release = undefined;
    await waitFor(() => ['en', 'ru', ''].every(language => !sortedNames(botApi.commandMenus.privateCommands(actor, language)).includes('video')), 'newer role revocation wins over held older refresh');
    await patch({ role: 'user', language: 'ru' });
    await menu(actor, ordinary, 'ru', 'latest role and locale restored after concurrent refresh');

    const stale = [{ command: 'status', description: 'Stale privileged override' }];
    for (const language_code of ['', 'en', 'ru']) botApi.commandMenus.set({ scope, language_code, commands: stale });
    await restartApp();
    await menu(actor, ordinary, 'ru', 'startup repairs persisted private scope and stale language overrides');
    assert.equal(await fixtureSql(`SELECT role||'|'||language FROM users WHERE id=${actor}`), 'user|ru');
    assert.equal(await fixtureSql(`SELECT count(*) FROM runtime_operations WHERE actor_id=${actor}`), '0', 'menu reconciliation cannot admit media operations');
    assert.equal(botApi.error, undefined);
  } finally {
    release?.();
    botApi.setMenuMode('success');
    if (savedPolicy) await fixtureSql(`UPDATE bot_settings SET value=${literal(savedPolicy)} WHERE key='bot_access_mode'`);
    else await fixtureSql("DELETE FROM bot_settings WHERE key='bot_access_mode'");
  }
  return 'implemented command catalog, owner isolation, role/ban/grant/language HTTP refresh, Bot API transient failures/retry_after, concurrent revocation and startup stale-scope recovery verified';
}
