import assert from 'node:assert/strict';
import test from 'node:test';
import { CommandMenus } from './command-menus.mjs';
import { BotApiFixture } from './bot-api.mjs';

const help = [{ command: 'help', description: 'Show help' }];
const video = [{ command: 'video', description: 'Download a video' }];
const russian = [{ command: 'help', description: 'Помощь' }];
const chat = { type: 'chat', chat_id: 42 };

test('menu scopes and languages have independent state and private precedence', () => {
  const menus = new CommandMenus();
  menus.set({ commands: help });
  menus.set({ scope: { type: 'all_private_chats' }, commands: video });
  menus.set({ scope: chat, commands: help });
  menus.set({ scope: chat, language_code: 'ru', commands: russian });
  assert.deepEqual(menus.privateCommands(42, 'ru'), russian);
  assert.deepEqual(menus.privateCommands(42, 'en'), help);
  assert.deepEqual(menus.privateCommands(43, 'ru'), video);
  menus.delete({ scope: chat, language_code: 'ru' });
  assert.deepEqual(menus.privateCommands(42, 'ru'), help);
  menus.delete({ scope: chat });
  assert.deepEqual(menus.privateCommands(42, 'ru'), video);
  assert.deepEqual(menus.get(), help);
  menus.get().push(...video);
  assert.deepEqual(menus.get(), help, 'returned lists cannot mutate fixture state');
});

test('menu validation rejects duplicate, malformed and oversized commands without changing state', () => {
  const menus = new CommandMenus();
  menus.set({ scope: chat, commands: help });
  for (const commands of [[...help, ...help], [{ command: '/help', description: 'Help' }], [{ command: 'help', description: '' }], [{ command: 'help', description: 'x'.repeat(257) }], Array.from({ length: 101 }, (_, i) => ({ command: `c${i}`, description: 'Help' }))]) {
    assert.throws(() => menus.set({ scope: chat, commands }));
    assert.deepEqual(menus.get({ scope: chat }), help);
  }
  assert.throws(() => menus.set({ scope: { type: 'chat' }, commands: help }));
  assert.throws(() => menus.set({ language_code: 'Russian', commands: help }));
});

test('Bot API menu transport supports state, targeted failures and delayed requests', async () => {
  const fixture = new BotApiFixture({ mediaBytes: Buffer.alloc(0) });
  const port = await fixture.start();
  const request = (method, data) => fetch(`http://127.0.0.1:${port}/botfixture/${method}`, {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(data),
  });
  try {
    assert.equal((await (await request('setMyCommands', { scope: chat, commands: help })).json()).result, true);
    assert.deepEqual((await (await request('getMyCommands', { scope: chat })).json()).result, help);
    fixture.setMenuMode('rate-limit-once', 42);
    assert.equal((await request('setMyCommands', { commands: video })).status, 200, 'global scope cannot consume a targeted failure');
    const limited = await request('setMyCommands', { scope: chat, commands: video });
    assert.equal(limited.status, 429);
    assert.equal((await limited.json()).parameters.retry_after, 2);
    assert.deepEqual(fixture.commandMenus.get({ scope: chat }), help);
    assert.equal((await request('setMyCommands', { scope: chat, commands: video })).status, 200);
    fixture.setMenuMode('api-error-once', 42);
    assert.equal((await request('deleteMyCommands', { scope: chat })).status, 500);
    assert.deepEqual(fixture.commandMenus.get({ scope: chat }), video);
    const release = fixture.holdNextMenu(42);
    const pending = request('setMyCommands', { scope: chat, language_code: 'ru', commands: russian });
    for (let i = 0; i < 100 && fixture.calls.at(-1)?.status !== 'hold-once'; i++) await new Promise(resolve => setTimeout(resolve, 10));
    assert.equal(fixture.calls.at(-1)?.status, 'hold-once');
    assert.equal(fixture.commandMenus.has({ scope: chat, language_code: 'ru' }), false);
    release();
    assert.equal((await pending).status, 200);
    assert.deepEqual(fixture.commandMenus.get({ scope: chat, language_code: 'ru' }), russian);
    fixture.setMenuMode('socket-drop-once', 42);
    await assert.rejects(request('setMyCommands', { scope: chat, commands: help }));
    assert.deepEqual(fixture.commandMenus.get({ scope: chat }), help, 'ambiguous response loss occurs after applying the menu');
    await request('deleteMyCommands', { scope: chat });
    assert.equal(fixture.commandMenus.has({ scope: chat }), false);
    assert.equal(fixture.error, undefined);
  } finally {
    await fixture.close();
  }
});
