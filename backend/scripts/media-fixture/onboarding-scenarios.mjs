import assert from 'node:assert/strict';
import { commandHarness } from './command-harness.mjs';

export async function runOnboardingScenarios(context) {
  const { botApi, fixtureSql, waitFor, restartApp, sourcePort, assertWorkspaceClean } = context;
  const h = commandHarness(context, 1_500_000);
  const openActor = 88001;
  const pendingActor = 88002;
  const directActor = 88003;
  let messageId = 1_600_000;
  const savedPolicy = await fixtureSql("SELECT value FROM bot_settings WHERE key='bot_access_mode'");
  const policy = value => fixtureSql(`INSERT INTO bot_settings(key,value,updated_at) VALUES('bot_access_mode','${value}',unixepoch('now')*1000000) ON CONFLICT(key) DO UPDATE SET value=excluded.value`);
  const operations = () => fixtureSql('SELECT count(*) FROM runtime_operations');
  const send = async (actor, command, language = 'en') => {
    const start = botApi.calls.length;
    botApi.enqueue(actor, actor, command, messageId++, language);
    return waitFor(() => botApi.calls.slice(start).find(c => c.method === 'sendmessage' && Number(c.data.chat_id) === actor), `onboarding ${actor} ${command.split(' ')[0]}`);
  };
  const row = actor => fixtureSql(`SELECT role||'|'||language||'|'||theme FROM users WHERE id=${actor}`);
  const literal = value => `'${String(value).replaceAll("'", "''")}'`;
  try {
    await policy('open');
    const welcome = await send(openActor, '/start');
    assert.match(welcome.data.text, /download|video|audio/i);
    assert.doesNotMatch(welcome.data.text, /\/cookie|\/proxy|\/uncache|\/status(?:\s|$)|\/runtime/i);
    assert.match(await row(openActor), /^user\|/);
    assert.equal(await fixtureSql(`SELECT first_name FROM users WHERE id=${openActor}`), 'Fixture');
    await fixtureSql(`UPDATE users SET language='ru',theme='mocha' WHERE id=${openActor}`);
    await fixtureSql(`INSERT INTO user_permissions(user_id,plugin,feature,granted_by) VALUES(${openActor},'test','preservation_only',42)`);
    const grants = await fixtureSql(`SELECT * FROM user_permissions WHERE user_id=${openActor}`);
    const before = await fixtureSql(`SELECT role||'|'||language||'|'||theme||'|'||updated_at FROM users WHERE id=${openActor}`);
    const help = await send(openActor, '/help', 'en');
    assert.match(help.data.text, /загруз|видео|аудио/i);
    assert.equal(await fixtureSql(`SELECT role||'|'||language||'|'||theme||'|'||updated_at FROM users WHERE id=${openActor}`), before, 'help must not invalidate identity-backed menus');
    await restartApp();
    assert.match((await send(openActor, '/help')).data.text, /загруз|видео|аудио/i);
    assert.equal(await fixtureSql(`SELECT * FROM user_permissions WHERE user_id=${openActor}`), grants, 'onboarding preserves explicit grants');

    const video = await h.submit(`/video http://127.0.0.1:${sourcePort}/media?case=ordinary-video`, { actor: openActor, chat: openActor });
    await h.succeeded(video.operationId);
    assert.equal(await fixtureSql(`SELECT actor_id FROM runtime_operations WHERE id='${video.operationId}'`), String(openActor));
    const upload = botApi.calls.slice(video.callStart).find(c => ['sendvideo', 'senddocument'].includes(c.method) && Number(c.chat_id) === openActor);
    assert(upload, 'ordinary-user video must reach their private chat');
    await h.replay(`/video http://127.0.0.1:${sourcePort}/media?case=ordinary-video`, video, { actor: openActor, chat: openActor });
    const audio = await h.execute('ordinary audio', `/audio http://127.0.0.1:${sourcePort}/media?case=ordinary-audio`, 'sendaudio',
      { video: false, audio: true, durationMin: 0.8, durationMax: 1.5 },
      { filename: 'audio.m4a', mimeType: 'audio/mp4' }, { actor: openActor, chat: openActor });
    assert(botApi.calls.slice(audio.callStart).some(c => c.method === 'sendaudio' && Number(c.chat_id) === openActor));

    const imageUrl = `http://127.0.0.1:${sourcePort}/image.png`;
    botApi.setExpectedUpload({ method: 'sendphoto', preserveMdat: false, filename: 'image.png', mimeType: 'image/png' });
    const image = await h.submit(`/image ${imageUrl}`, { actor: openActor, chat: openActor });
    await h.succeeded(image.operationId);
    assert(botApi.calls.slice(image.callStart).some(c => c.method === 'sendphoto' && Number(c.chat_id) === openActor));
    await h.replay(`/image ${imageUrl}`, image, { actor: openActor, chat: openActor });
    botApi.setExpectedUpload(null);
    await assertWorkspaceClean();

    const directUrl = `http://127.0.0.1:${sourcePort}/media?case=direct-onboarding`;
    const direct = await h.submit(directUrl, { actor: directActor, chat: directActor, entities: [{ type: 'url', offset: 0, length: directUrl.length }] });
    await h.succeeded(direct.operationId);
    assert.match(await row(directActor), /^user\|/, 'a download can onboard without a prior WebApp or start command');

    await policy('approved_only');
    const pending = await send(pendingActor, '/start');
    assert.match(pending.data.text, /approval|approved|pending|access/i);
    assert.match(await row(pendingActor), /^restricted\|/);
    const count = await operations();
    await send(pendingActor, `/video http://127.0.0.1:${sourcePort}/media?case=pending`);
    assert.equal(await operations(), count);
    assert.match(await row(openActor), /^user\|ru\|mocha$/, 'policy changes do not demote existing users or overwrite preferences');
    await fixtureSql(`UPDATE users SET role='user' WHERE id=${pendingActor}`);
    assert.match((await send(pendingActor, '/start')).data.text, /download|video|audio/i);

    await fixtureSql(`UPDATE users SET ban_until=unixepoch('now')*1000000+600000000 WHERE id=${pendingActor}`);
    const bannedBefore = await fixtureSql(`SELECT * FROM users WHERE id=${pendingActor}`);
    const bannedCount = await operations();
    const denied = await send(pendingActor, '/start');
    assert.doesNotMatch(denied.data.text, /\/video|\/cookie|\/proxy/);
    await send(pendingActor, `/video http://127.0.0.1:${sourcePort}/media?case=banned`);
    assert.equal(await operations(), bannedCount);
    assert.equal(await fixtureSql(`SELECT * FROM users WHERE id=${pendingActor}`), bannedBefore, 'blocked onboarding cannot rewrite the account');

    const controlCount = await operations();
    const cookies = await fixtureSql('SELECT * FROM cookies ORDER BY id');
    const settings = await fixtureSql(`SELECT * FROM dl_user_settings WHERE user_id=${openActor}`);
    const controlStart = botApi.calls.length;
    for (const command of [
      '/status', '/uncache', '/proxy', '/cookie',
    ]) {
      botApi.enqueue(openActor, openActor, command, messageId++);
    }
    await send(openActor, '/help');
    assert.equal(await operations(), controlCount, 'ordinary users cannot admit credential or admin operations');
    assert.equal(await fixtureSql('SELECT * FROM cookies ORDER BY id'), cookies);
    assert.equal(await fixtureSql(`SELECT * FROM dl_user_settings WHERE user_id=${openActor}`), settings);
    const controlReplies = botApi.calls.slice(controlStart).filter(c => c.method === 'sendmessage');
    assert.equal(controlReplies.length, 1, 'ordinary-user credential and admin commands must be silent');
    assert(!botApi.calls.slice(controlStart).some(c => c.data?.reply_markup), 'owner-private controls must not expose menus');
    assert.equal(await fixtureSql(`SELECT count(*) FROM runtime_cut_sessions WHERE actor_id=${openActor}`), '0');

    await fixtureSql(`UPDATE users SET role='restricted' WHERE id=${openActor}`);
    const revokedCount = await operations();
    await restartApp();
    await send(openActor, `/video http://127.0.0.1:${sourcePort}/media?case=revoked`);
    assert.equal(await operations(), revokedCount, 'persisted role revocation must stop admission after restart');
  } finally {
    botApi.setExpectedUpload(null);
    if (savedPolicy) {
      await fixtureSql(`UPDATE bot_settings SET value=${literal(savedPolicy)} WHERE key='bot_access_mode'`);
    } else {
      await fixtureSql("DELETE FROM bot_settings WHERE key='bot_access_mode'");
    }
  }
  return 'ordinary private registration/download/replay/restart, approved-only admission, bans/revocation and owner-control isolation verified';
}
