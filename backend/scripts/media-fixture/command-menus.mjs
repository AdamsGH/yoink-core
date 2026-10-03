import assert from 'node:assert/strict';

function scopeKey(scope = { type: 'default' }) {
  assert(scope && typeof scope === 'object' && !Array.isArray(scope), 'command menu scope must be an object');
  assert(['default', 'all_private_chats', 'all_group_chats', 'all_chat_administrators', 'chat', 'chat_administrators', 'chat_member'].includes(scope.type), 'unsupported command menu scope');
  const key = [scope.type];
  if (['chat', 'chat_administrators', 'chat_member'].includes(scope.type)) {
    assert(Number.isSafeInteger(scope.chat_id) && scope.chat_id !== 0, 'command menu scope needs a numeric chat identifier');
    key.push(scope.chat_id);
  }
  if (scope.type === 'chat_member') {
    assert(Number.isSafeInteger(scope.user_id) && scope.user_id > 0, 'command menu scope needs a positive member identifier');
    key.push(scope.user_id);
  }
  return key.join(':');
}

export class CommandMenus {
  #menus = new Map();

  #key(scope, language = '') {
    assert(typeof language === 'string' && (language === '' || /^[a-z]{2}$/.test(language)), 'command menu language must be empty or a two-letter ISO code');
    return `${scopeKey(scope)}:${language}`;
  }

  set({ scope, language_code, commands }) {
    assert(Array.isArray(commands) && commands.length <= 100, 'command menu must contain at most 100 commands');
    const names = new Set();
    for (const { command, description } of commands) {
      assert(typeof command === 'string' && /^[a-z0-9_]{1,32}$/.test(command), 'command menu name is invalid');
      assert(typeof description === 'string' && [...description].length >= 1 && [...description].length <= 256, 'command menu description must contain 1..256 characters');
      assert(!names.has(command), 'command menu must not repeat command names');
      names.add(command);
    }
    this.#menus.set(this.#key(scope, language_code), structuredClone(commands));
  }

  delete({ scope, language_code }) {
    this.#menus.delete(this.#key(scope, language_code));
  }

  get({ scope, language_code } = {}) {
    return structuredClone(this.#menus.get(this.#key(scope, language_code)) ?? []);
  }

  has({ scope, language_code } = {}) {
    return this.#menus.has(this.#key(scope, language_code));
  }

  privateCommands(chatId, language = '') {
    for (const scope of [{ type: 'chat', chat_id: chatId }, { type: 'all_private_chats' }, { type: 'default' }]) {
      for (const language_code of [...new Set([language, ''])]) {
        if (this.has({ scope, language_code })) return this.get({ scope, language_code });
      }
    }
    return [];
  }
}
