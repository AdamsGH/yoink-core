import assert from 'node:assert/strict';
import { setTimeout as sleep } from 'node:timers/promises';
import { commandHarness } from './command-harness.mjs';

export async function runUncacheScenarios(context) {
  const { botApi, sourcePort, fixtureSql, waitFor, restartApp, assertWorkspaceClean } = context;
  const h = commandHarness(context, 24000);
  const source = `http://127.0.0.1:${sourcePort}/media?case=uncache-private-marker`;
  const otherSource = `${source}&other=1`;
  const video = url => `/video --height best --video-codec h264 --container mp4 --send-as-file false ${url}`;
  const audio = `/audio --audio-codec mp3 --send-as-file false ${source}`;
  const sourceKeys = url => `SELECT cache_key FROM runtime_operations WHERE actor_id=42 AND json_extract(request,'$.target.bot_id')=123456 AND json_extract(request,'$.source_url')='${url}'`;
  const cached = url => fixtureSql(`SELECT count(*) FROM runtime_media_cache WHERE actor_id=42 AND bot_id=123456 AND cache_key IN (${sourceKeys(url)})`);
  const count = () => fixtureSql('SELECT count(*) FROM runtime_operations');
  const sends = () => botApi.calls.filter(call => call.method === 'sendmessage');
  const videoExpectation = { video: true, audio: true, durationMin: 0.8, durationMax: 1.5 };
  const videoContract = { filename: 'media.mp4', mimeType: 'video/mp4' };
  let nextMessage = 30000;
  let cloneKey;
  const cloneWhere = key => `cache_key='${key}' AND ((actor_id=8000000000099 AND bot_id=123456) OR (actor_id=42 AND bot_id=987654321001))`;
  async function command(text, prefix, { messageId = nextMessage++, language = 'en' } = {}) {
    const from = botApi.calls.length;
    botApi.enqueue(42, 42, text, messageId, language);
    const reply = await waitFor(() => botApi.calls.slice(from).find(call => call.method === 'sendmessage'
      && Number(call.data.chat_id) === 42 && call.data.text.startsWith(prefix)), 'uncache command response');
    assert(!reply.data.text.includes('uncache-private-marker'), 'cache replies must redact source URLs');
    return { messageId, reply };
  }
  const savedBan = await fixtureSql('SELECT COALESCE(CAST(ban_until AS TEXT),\'NULL\') FROM users WHERE id=42');
  try {
    const first = await h.execute('uncache original video', video(source), 'sendvideo', videoExpectation, videoContract);
    const second = await h.execute('uncache audio variant', audio, 'sendaudio',
      { video: false, audio: true, audioCodec: 'mp3' },
      { filename: 'audio.mp3', mimeType: 'audio/mpeg' });
    const control = await h.execute('uncache distinct URL', video(otherSource), 'sendvideo', videoExpectation, videoContract);
    await waitFor(async () => await cached(source) === '2' && await cached(otherSource) === '1', 'published URL variants');
    const hits = botApi.cachedCalls;
    const hit = await h.submit(video(source));
    await h.succeeded(hit.operationId);
    assert.equal(botApi.cachedCalls, hits + 1, 'control must prove cache hit before clearing');
    const candidateKey = await fixtureSql(`SELECT cache_key FROM runtime_operations WHERE id='${first.operationId}'`);
    assert.match(candidateKey, /^v1:[a-f0-9]{64}$/);
    assert.equal(await fixtureSql(`SELECT count(*) FROM runtime_media_cache WHERE ${cloneWhere(candidateKey)}`), '0');
    cloneKey = candidateKey;
    // Isolated foreign partitions retain exactly the same key to catch unscoped deletes.
    for (const [actor, bot] of [[8000000000099, 123456], [42, 987654321001]]) {
      await fixtureSql(`INSERT INTO runtime_media_cache(actor_id,bot_id,cache_key,revision,media,expires_at,created_at) SELECT ${actor},${bot},cache_key,revision,media,expires_at,created_at FROM runtime_media_cache WHERE actor_id=42 AND bot_id=123456 AND cache_key='${cloneKey}'`);
    }
    const foreign = () => fixtureSql(`SELECT actor_id,bot_id,cache_key,revision,media,expires_at,created_at FROM runtime_media_cache WHERE ${cloneWhere(cloneKey)} ORDER BY actor_id,bot_id`);
    const savedForeign = await foreign();
    const originalIds = [first, second, control, hit].map(item => `'${item.operationId}'`).join(',');
    const operations = () => fixtureSql(`SELECT * FROM runtime_operations WHERE id IN (${originalIds}) ORDER BY id`);
    const deliveries = () => fixtureSql(`SELECT * FROM runtime_delivery_units WHERE operation_id IN (${originalIds}) ORDER BY operation_id,attempt,ordinal`);
    const savedOperations = await operations(), savedDeliveries = await deliveries(), beforeClear = await count();
    for (const text of ['/uncache', '/uncache all', `/uncache ${source} extra`, '/uncache https://name:password@example.test/']) {
      await command(text, 'Use /uncache', { messageId: 39000 });
    }
    assert.equal(await cached(source), '2');
    const beforeSilent = sends().length;
    for (const [actor, chat, text] of [[99, 99, `/uncache ${source}`], [42, -10042, `/uncache ${source}`], [42, 42, `/uncache@other_bot ${source}`]]) {
      botApi.enqueue(actor, chat, text, 39001);
    }
    await command('/help', 'Available:', { messageId: 39002 });
    await sleep(300);
    assert.equal(sends().length, beforeSilent + 1, 'non-owner/group/foreign-bot commands stay silent');
    await fixtureSql("UPDATE users SET ban_until=unixepoch('now')*1000000+600000000 WHERE id=42");
    await command(`/uncache ${source}`, 'This operation is not allowed.', { messageId: 39003 });
    assert.equal(await cached(source), '2');
    await fixtureSql(`UPDATE users SET ban_until=${savedBan} WHERE id=42`);

    const cleared = await command(`/uncache ${source}`, 'Removed 2 cached batches');
    assert.equal(await cached(source), '0');
    assert.equal(await cached(otherSource), '1');
    assert.equal(await foreign(), savedForeign);
    assert.equal(await count(), beforeClear, 'uncache is not a media operation');
    assert.equal(await operations(), savedOperations);
    assert.equal(await deliveries(), savedDeliveries);
    await h.replay(video(source), first);
    assert.equal(await cached(source), '0', 'replaying a completed job does not recreate its cache');
    await h.execute('uncache forces fresh upload', video(source), 'sendvideo', videoExpectation, videoContract);
    await waitFor(async () => await cached(source) === '1', 'repopulated cache');
    await restartApp();
    await command(`/uncache ${source}`, 'This cache-clear request', { messageId: cleared.messageId });
    await command(`/uncache ${otherSource}`, 'This cache-clear request', { messageId: cleared.messageId });
    await command(`/uncache ${source}`, 'This cache-clear request', { messageId: cleared.messageId - 1 });
    assert.equal(await cached(source), '1', 'old updates after restart cannot delete repopulated cache');
    assert.equal(await cached(otherSource), '1');
    await command(`/uncache ${source}`, 'Из вашего кэша удалено наборов файлов для этой ссылки: 1.', { language: 'ru' });
    await command(`/uncache ${source}`, 'No cached batches');
    assert.equal(await cached(source), '0');
    assert.equal(await foreign(), savedForeign);
    assert.equal(await cached(otherSource), '1');
    assert.equal(await operations(), savedOperations);
    assert.equal(await deliveries(), savedDeliveries);
    await assertWorkspaceClean();
  } finally {
    botApi.setExpectedUpload(null);
    await fixtureSql(`UPDATE users SET ban_until=${savedBan} WHERE id=42`);
    if (cloneKey) await fixtureSql(`DELETE FROM runtime_media_cache WHERE ${cloneWhere(cloneKey)}`);
  }
  return 'actor/bot/exact-URL cache clear across formats, untouched jobs/deliveries/foreign partitions, fresh upload, stale-update fencing across repopulation/restart, strict grammar, bans and EN/RU replies passed';
}
