import assert from 'node:assert/strict';
import { commandHarness } from './command-harness.mjs';
import { galleryImage } from './gallery-source.mjs';

// Exact opt-in scenario: real Telegram adapter, durable queue, yt-dlp/gallery-dl,
// uploads and ffprobe from existing fixtures. Coordinator runs after image builds.
export async function runDeferredMediaScenarios(context) {
  const { botApi, sourcePort, fixtureSql, waitFor, restartApp, restartGallery, assertWorkspaceClean, proxyUrl } = context;
  const h = commandHarness(context, 31000);
  let commandId = 30000;
  const entity = url => ({ entities: [{ type: 'url', offset: 0, length: url.length }] });
  async function arm(mode, messageId = commandId++, actor = 42, chat = 42) {
    const start = botApi.calls.length;
    botApi.enqueue(actor, chat, `/${mode}`, messageId);
    await waitFor(() => botApi.calls.slice(start).some(c => c.method === 'sendmessage'), 'deferred media command acknowledgement');
    return messageId;
  }
  const state = () => fixtureSql('SELECT COALESCE(mode,\'consumed\') FROM runtime_deferred_media WHERE actor_id=42 AND bot_id=123456 AND chat_id=42');
  const source = `http://127.0.0.1:${sourcePort}/media?case=deferred-media`;
  const count = () => fixtureSql('SELECT count(*) FROM runtime_operations');
  try {
    const firstCommand = await arm('audio');
    assert.equal(await state(), 'audio');
    botApi.enqueue(42,42,'not a URL',commandId++);
    const helpStart = botApi.calls.length;
    botApi.enqueue(42,42,'/help',commandId++);
    await waitFor(() => botApi.calls.slice(helpStart).some(c => c.method === 'sendmessage'), 'non-URL synchronization barrier');
    assert.equal(await state(),'audio');
    await restartApp();
    const audio = await h.execute('deferred audio', source, 'sendaudio', { video: false, audio: true }, { filename: 'audio.m4a', mimeType: 'audio/mp4' }, entity(source));
    const frozen = await h.request(audio.operationId);
    assert.equal(frozen.media_mode,'audio');
    assert.equal(await state(),'consumed');
    await arm('audio', firstCommand);
    assert.equal(await state(),'consumed','old command cannot recreate consumed arm');
    commandId = 32000;
    await arm('image');
    await h.replay(source,audio,entity(source));
    assert.deepEqual(await h.request(audio.operationId),frozen);
    assert.equal(await state(),'image','URL replay must preserve newer arm');
    await arm('video');
    assert.equal(await state(),'video');
    const video = await h.submit(`${source}&video=1`,{ messageId:33000, ...entity(`${source}&video=1`) });
    await h.succeeded(video.operationId);
    assert.equal((await h.request(video.operationId)).media_mode,'video');
    const defaults = await h.submit(`${source}&default=1`,{ messageId:33001, ...entity(`${source}&default=1`) });
    await h.succeeded(defaults.operationId);
    assert.equal((await h.request(defaults.operationId)).media_mode,'video');
    commandId = 34000;
    await arm('image');
    const before = await count();
    const start = botApi.calls.length;
    botApi.enqueue(42,42,'/audio not-a-url',commandId++);
    await waitFor(() => botApi.calls.slice(start).some(c=>c.method==='sendmessage'),'invalid immediate command reply');
    assert.equal(await count(),before); assert.equal(await state(),'image');
    const literal = value => "'" + value.replaceAll("'", "''") + "'";
    await fixtureSql(`INSERT INTO dl_user_settings (user_id, quality, codec, container, proxy_enabled, proxy_url, keyboard, subs_enabled, subs_auto, subs_always_ask, subs_lang, split_size, nsfw_blur, mediainfo, send_as_file, audio_codec, gallery_zip, use_pool_cookies, youtube_auth_mode, args_json, created_at, updated_at) VALUES (42, 'best', 'avc1', 'mp4', 1, ${literal(proxyUrl)}, '2x3', 0, 0, 0, 'en', 2043000000, 1, 0, 0, 'best', 0, 1, 'cookies', '{}', unixepoch('now')*1000000, unixepoch('now')*1000000) ON CONFLICT(user_id) DO UPDATE SET proxy_enabled=1, proxy_url=excluded.proxy_url, send_as_file=0, gallery_zip=0`);
    await restartGallery(true);
    await restartApp();
    const gallery = 'https://imgbox.com/g/Gallery003';
    botApi.setExpectedAlbum(Array.from({length:3},(_,i)=>({kind:'photo',filename:'image.png',mimeType:'image/png',bytes:galleryImage(i+1)})));
    const image = await h.submit(gallery,{ messageId:35000, ...entity(gallery) });
    await h.succeeded(image.operationId);
    assert.equal((await h.request(image.operationId)).media_mode,'image');
    assert.deepEqual(botApi.lastGroupBytes,Array.from({length:3},(_,i)=>galleryImage(i+1)));
    assert.equal(await state(),'consumed');
    await restartApp();
    await h.replay(gallery,image,entity(gallery));
    await assertWorkspaceClean();
  } finally { botApi.setExpectedUpload(null); botApi.setExpectedAlbum(null); }
  return 'deferred audio/image real output, video reset/defaults, restart, non-URL/rejection preservation, command fencing and immutable URL replay passed';
}
