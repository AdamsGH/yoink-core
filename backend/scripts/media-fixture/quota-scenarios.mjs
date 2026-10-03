import assert from 'node:assert/strict';
import { commandHarness } from './command-harness.mjs';

export async function runQuotaScenarios(context) {
  const { botApi, fixtureSql, waitFor, restartApp, sourcePort, assertWorkspaceClean } = context;
  const h = commandHarness(context, 2_600_000);
  const actor = 89101;
  const russianActor = 89102;
  const blockedActor = 89103;
  let messageId = 2_700_000;
  const url = `http://127.0.0.1:${sourcePort}/media?case=quota`;
  const command = `/video ${url}`;
  const literal = value => `'${String(value).replaceAll("'", "''")}'`;
  const savedPolicy = await fixtureSql("SELECT value FROM bot_settings WHERE key='bot_access_mode'");
  const snapshot = id => fixtureSql(`SELECT window||'|'||count||'|'||reset_at FROM rate_limits WHERE user_id=${id} ORDER BY window`);
  const operationCount = () => fixtureSql('SELECT count(*) FROM runtime_operations');
  const send = async (id, text, language = 'en') => {
    const start = botApi.calls.length;
    botApi.enqueue(id, id, text, messageId++, language);
    return waitFor(() => botApi.calls.slice(start).find(call =>
      call.method === 'sendmessage' && Number(call.data.chat_id) === id), `quota reply for ${id}`);
  };
  const reject = async (id, language, expected) => {
    const operations = await operationCount();
    const counters = await snapshot(id);
    const reply = await send(id, command, language);
    assert.match(reply.data.text, expected);
    assert.equal(await operationCount(), operations, 'quota rejection must not create an operation');
    assert.equal(await snapshot(id), counters, 'quota rejection must not modify any window');
  };
  try {
    await fixtureSql("INSERT INTO bot_settings(key,value,updated_at) VALUES('bot_access_mode','open',unixepoch('now')*1000000) ON CONFLICT(key) DO UPDATE SET value=excluded.value");
    await send(actor, '/start');
    assert.equal(await snapshot(actor), '', 'onboarding must not consume download quota');
    const first = await h.submit(command, { actor, chat: actor });
    await h.succeeded(first.operationId);
    for (let index = 1; index < 5; index++) {
      const next = await h.submit(command, { actor, chat: actor });
      await h.succeeded(next.operationId);
      assert.equal(await fixtureSql(`SELECT attempt FROM runtime_operations WHERE id='${next.operationId}'`), '1');
    }
    assert.equal(await fixtureSql(`SELECT count(*) FROM rate_limits WHERE user_id=${actor} AND count=5`), '3', 'new cache-served operations each consume one quota unit');
    await reject(actor, 'en', /limit|quota|wait|minute/i);
    const full = await snapshot(actor);
    await h.replay(command, first, { actor, chat: actor });
    assert.equal(await snapshot(actor), full, 'replay at exhausted quota must not charge');
    await restartApp();
    assert.equal(await snapshot(actor), full, 'quota counters must survive restart');
    await reject(actor, 'en', /limit|quota|wait|minute/i);
    await h.replay(command, first, { actor, chat: actor });
    assert.equal(await snapshot(actor), full, 'replay after restart must not charge');

    await fixtureSql(`UPDATE rate_limits SET reset_at=0 WHERE user_id=${actor} AND window='minute'`);
    const resumed = await h.submit(command, { actor, chat: actor });
    await h.succeeded(resumed.operationId);
    assert.equal(await fixtureSql(`SELECT count FROM rate_limits WHERE user_id=${actor} AND window='minute'`), '1');
    assert.equal(await fixtureSql(`SELECT count(*) FROM rate_limits WHERE user_id=${actor} AND window IN ('hour','day') AND count=6`), '2', 'minute reset must not reset the longer windows');

    await send(russianActor, '/start', 'ru');
    await fixtureSql(`UPDATE users SET language='ru' WHERE id=${russianActor}`);
    await fixtureSql(`INSERT INTO rate_limits(user_id,window,count,reset_at) VALUES(${russianActor},'minute',5,unixepoch('now')*1000000+600000000)`);
    await reject(russianActor, 'en', /лимит|подожд|минут|квот/i);
    await fixtureSql(`UPDATE users SET role='admin' WHERE id=${russianActor}`);
    await reject(russianActor, 'ru', /лимит|подожд|минут|квот/i);

    await send(blockedActor, '/start');
    await fixtureSql(`UPDATE users SET role='restricted' WHERE id=${blockedActor}`);
    const operations = await operationCount();
    await send(blockedActor, command);
    assert.equal(await operationCount(), operations, 'authorization denial must not create an operation');
    assert.equal(await snapshot(blockedActor), '', 'authorization denial must not consume quota');

    const ownerBefore = await snapshot(42);
    for (let index = 0; index < 6; index++) {
      const owner = await h.submit(command);
      await h.succeeded(owner.operationId);
    }
    assert.equal(await snapshot(42), ownerBefore, 'configured owner must not consume time quota');
    await assertWorkspaceClean();
  } finally {
    botApi.setExpectedUpload(null);
    if (savedPolicy) {
      await fixtureSql(`UPDATE bot_settings SET value=${literal(savedPolicy)} WHERE key='bot_access_mode'`);
    } else {
      await fixtureSql("DELETE FROM bot_settings WHERE key='bot_access_mode'");
    }
  }
  return 'anchored user quotas, cache charges, non-charging rejection, replay/restart, localized errors, role isolation and owner exemption verified';
}
