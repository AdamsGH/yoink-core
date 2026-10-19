import assert from 'node:assert/strict';
import { setTimeout as sleep } from 'node:timers/promises';
import { commandHarness, admissionNotice } from './command-harness.mjs';
import { galleryImage } from './gallery-source.mjs';

// Native polling -> confined real extraction -> answer -> actual selection.
export async function runInlineScenarios(context) {
  const { botApi, fixtureSql, waitFor, sourcePort, sourceFixture, inlineSource, restartInlineAppGracefully, waitForInlineWorkspaceClean, queryJob, assertWorkspaceClean } = context;
  const h = commandHarness(context, 41000);
  const user = 8000000000011, restricted = 8000000000012, group = -8000000000011;
  let messageId = 40000, queryId = 0;
  const visible = text => ({ entities: [{ type: 'url', offset: 0, length: text.split(' ')[0].length }], viaBot: 123456 });
  const selection = text => ({ ...visible(text), messageId: messageId++ });
  const counts = () => fixtureSql("SELECT json_object('operations',(SELECT count(*) FROM runtime_operations),'quotas',(SELECT coalesce(json_group_array(json_object('user',user_id,'window',window,'count',count,'reset',reset_at)),'[]') FROM rate_limits),'media',(SELECT coalesce(json_group_array(json_object('actor',actor_id,'message',message_id,'mode',mode)),'[]') FROM runtime_deferred_media),'nsfw',(SELECT coalesce(json_group_array(actor_id),'[]') FROM runtime_nsfw_marks),'marks',(SELECT count(*) FROM runtime_nsfw_mark_events),'quality',(SELECT coalesce(json_group_array(request_json),'[]') FROM runtime_quality_sessions),'cut',(SELECT coalesce(json_group_array(source_url),'[]') FROM runtime_cut_sessions))");
  async function lookup(query, actor = 42, options = {}) {
    await sleep(70);
    const id = botApi.enqueueInline(actor, query, { id: `native-inline-${++queryId}`, ...options });
    return waitFor(() => botApi.calls.find(call => call.method === 'answerinlinequery' && call.data.inline_query_id === id), 'personal inline answer');
  }
  async function command(text, actor = 42) {
    const start = botApi.calls.length;
    botApi.enqueue(actor, actor, text, messageId++);
    return waitFor(() => botApi.calls.slice(start).find(call => call.method === 'sendmessage' && Number(call.data.chat_id) === actor), 'private inline selection control');
  }
  const mediaState = () => fixtureSql("SELECT coalesce(mode,'consumed') FROM runtime_deferred_media WHERE actor_id=42 AND bot_id=123456 AND chat_id=42");
  const operationCount = () => fixtureSql('SELECT count(*) FROM runtime_operations');
  try {
    // Selections use the existing user's explicit proxy preference. Search has
    // only operator egress authority, independent of this row and its cookies.
    for (const actor of [42,user]) await fixtureSql(`INSERT INTO dl_user_settings(user_id,proxy_enabled,created_at,updated_at) VALUES (${actor},1,unixepoch('now')*1000000,unixepoch('now')*1000000) ON CONFLICT(user_id) DO UPDATE SET proxy_enabled=1,proxy_url=NULL,quality='best',nsfw_blur=1`);
    await command('/audio'); await command('/nsfw mark');
    const armed = await counts();
    const beforeSource = sourceFixture.mediaRequests;
    // Search ignores personal proxy and unsupported per-user YouTube auth mode.
    await fixtureSql("UPDATE dl_user_settings SET proxy_enabled=1,proxy_url='http://user:secret@127.0.0.1:1',youtube_auth_mode='oauth' WHERE user_id=42");
    const eight = await lookup('cats');
    await fixtureSql("UPDATE dl_user_settings SET proxy_url=NULL,youtube_auth_mode='cookies' WHERE user_id=42");
    assert.equal(eight.data.results.length,8);
    assert.match(eight.data.results[0].title,/Unicode/);
    assert.match(eight.data.results[0].description,/Fixture channel/);
    assert.match(eight.data.results[0].description,/1,?234/);
    const searchUrl = eight.data.results[0].input_message_content.message_text;
    assert.equal(searchUrl,'https://www.youtube.com/watch?v=fixture0000');
    for (const [query, count] of [['zero',0], ['duplicate',1], ['oversized',8], ['http2 explained',8], ['http proxy setup',8]]) {
      assert.equal((await lookup(query)).data.results.length,count);
    }
    for (const query of ['', 'x', 'x'.repeat(257), 'https://', 'https://user:secret@www.youtube.com/watch?v=fixture0000', 'https://spotify.com/track/a', 'failure', 'network denial', 'response overflow']) {
      assert.equal((await lookup(query)).data.results.length,0);
    }
    assert.equal(sourceFixture.mediaRequests,beforeSource,'query cannot download media');
    assert.equal(await counts(),armed,'queries cannot consume media/NSFW arms, quota, operations or chooser state');
    botApi.setInlineMode('failure-once'); await lookup(searchUrl);
    assert.equal(await counts(),armed,'answer failure cannot mutate state');
    assert(botApi.polls.some(poll => poll.allowed_updates.includes('inline_query')));

    // Current actor feature grants, no invented group or topic authority.
    assert.equal((await lookup('cats',restricted,{chatType:'supergroup'})).data.results.length,8);
    const help = await command('/start search_help',restricted);
    assert(/YouTube/.test(help.data.text));
    await fixtureSql(`DELETE FROM user_permissions WHERE user_id=${restricted} AND plugin='dl' AND feature='inline'`);
    assert.equal((await lookup('cats',restricted)).data.results.length,0);
    assert(!/YouTube/.test((await command('/start search_help',restricted)).data.text));
    for (const actor of [777777,8000000000013,8000000000014]) {
      const trace = inlineSource.trace.length;
      const denied = await lookup('cats',actor,{chatType:'supergroup'});
      assert.equal(denied.data.results.length,0); assert.equal(denied.data.button,undefined);
      assert.equal(inlineSource.trace.length,trace,'denied actor must not reach worker');
    }
    await fixtureSql("UPDATE users SET ban_until=(unixepoch('now')+3600)*1000000 WHERE id=42");
    assert.equal((await lookup('cats')).data.results.length,0,'configured owner cannot bypass active ban');
    await fixtureSql('UPDATE users SET ban_until=NULL WHERE id=42');

    // Revocation while real HTTP extraction is held must empty the later answer.
    await fixtureSql(`INSERT INTO user_permissions(user_id,plugin,feature,granted_by,granted_at,grant_source) VALUES (${restricted},'dl','inline',42,unixepoch('now')*1000000,'manual')`);
    const release = inlineSource.hold('revoked');
    const heldId = botApi.enqueueInline(restricted,'revoked',{id:`native-inline-${++queryId}`});
    await waitFor(() => inlineSource.trace.some(entry => entry.query === 'revoked'),'held real YouTube lookup');
    await fixtureSql(`DELETE FROM user_permissions WHERE user_id=${restricted} AND plugin='dl' AND feature='inline'`);
    release();
    const revoked = await waitFor(() => botApi.calls.find(call => call.method==='answerinlinequery' && call.data.inline_query_id===heldId),'reauthorized revoked answer');
    assert.deepEqual(revoked.data.results,[]); assert.equal(revoked.data.button,undefined);

    // Supersession and responsiveness: /help remains live while worker waits.
    const releaseSlow = inlineSource.hold('superseded');
    const oldId = botApi.enqueueInline(42,'superseded',{id:`native-inline-${++queryId}`});
    await waitFor(() => inlineSource.trace.some(entry=>entry.query==='superseded'),'slow real search entered');
    await command('/help');
    const newer = await lookup(searchUrl);
    assert.equal(newer.data.results.length,1);
    releaseSlow(); await sleep(200);
    assert(!botApi.calls.some(call=>call.method==='answerinlinequery' && call.data.inline_query_id===oldId),'superseded lookup cannot answer');
    const gates = [inlineSource.hold('concurrent1'), inlineSource.hold('concurrent2'), inlineSource.hold('concurrent3')];
    const actors = [42,user,8000000000017];
    for (let i=0;i<actors.length;i++) botApi.enqueueInline(actors[i],`concurrent${i+1}`,{id:`native-inline-${++queryId}`});
    await waitFor(()=>[1,2,3].every(index=>inlineSource.trace.some(entry=>entry.query===`concurrent${index}`)),'three concurrent confined searches');
    const overflow = await lookup('capacity',8000000000015);
    assert.equal(overflow.data.results.length,0);
    assert(!inlineSource.trace.some(entry=>entry.query==='capacity'),'fourth concurrent search must not spawn a worker');
    for (let i=0;i<actors.length;i++) await lookup(searchUrl,actors[i]);
    gates.forEach(release=>release());
    const firstBurst=botApi.enqueueInline(42,'burst1',{id:`native-inline-${++queryId}`});
    const lastBurst=botApi.enqueueInline(42,'burst2',{id:`native-inline-${++queryId}`});
    const throttled=await waitFor(()=>botApi.calls.find(call=>call.method==='answerinlinequery' && call.data.inline_query_id===lastBurst),'per-actor inline request rate');
    assert.equal(throttled.data.results.length,0);
    assert(!botApi.calls.some(call=>call.method==='answerinlinequery' && call.data.inline_query_id===firstBurst),'rate-limited newer query still supersedes');
    const releaseTimeout = inlineSource.hold('timeout');
    await sleep(70);
    const timeoutId=botApi.enqueueInline(42,'timeout',{id:`native-inline-${++queryId}`});
    await waitFor(()=>inlineSource.trace.some(entry=>entry.query==='timeout'),'deadline search entered while extraction held');
    // An exhausted end-to-end budget cannot start a fresh answer budget. The
    // original workspace disappears only after executor cancellation/reaping.
    await waitForInlineWorkspaceClean();
    await assertWorkspaceClean();
    assert(!botApi.calls.some(call=>call.method==='answerinlinequery' && call.data.inline_query_id===timeoutId),'expired query must not answer with a renewed budget');
    releaseTimeout();
    assert.equal(await counts(),armed);

    // Pending audio and NSFW intent are consumed only by a real URL selection.
    const audio = await h.execute('inline deferred audio',searchUrl,'sendaudio',{video:false,audio:true},{filename:'audio.m4a',mimeType:'audio/mp4'},selection(searchUrl));
    assert.equal((await h.request(audio.operationId)).media_mode,'audio');
    assert.equal(await mediaState(),'consumed');
    assert.equal(await fixtureSql('SELECT count(*) FROM runtime_nsfw_marks WHERE actor_id=42'),'0');
    const frozen = await h.request(audio.operationId);
    await command('/image');
    await h.replay(searchUrl,audio,visible(searchUrl));
    assert.deepEqual(await h.request(audio.operationId),frozen);
    assert.equal(await mediaState(),'image','immutable selection replay cannot consume newer arm');
    const gallery='https://imgbox.com/g/Gallery003';
    const article = await lookup(gallery);
    assert.equal(article.data.results[0].input_message_content.message_text,gallery);
    botApi.setExpectedAlbum(Array.from({length:3},(_,i)=>({kind:'photo',filename:'image.png',mimeType:'image/png',bytes:galleryImage(i+1)})));
    const image=await h.submit(gallery,selection(gallery)); await h.succeeded(image.operationId);
    assert.equal((await h.request(image.operationId)).media_mode,'image'); assert.equal(await mediaState(),'consumed'); botApi.setExpectedAlbum(null);

    // Direct article selections use ordinary compatible cache and current policy.
    const direct=`http://127.0.0.1:${sourcePort}/content-page.html?case=inline-cache`;
    const selected=(await lookup(direct)).data.results[0].input_message_content.message_text;
    const first=await h.submit(selected,selection(selected)); await h.succeeded(first.operationId);
    const reads=sourceFixture.mediaRequests, uploads=botApi.uploadCalls;
    const cachedArticle=await lookup(direct); assert.equal(cachedArticle.data.results[0].type,'article');
    const cache=await h.submit(selected,selection(selected)); await h.succeeded(cache.operationId);
    assert.equal(botApi.uploadCalls,uploads); assert.equal(sourceFixture.mediaRequests,reads);
    assert(botApi.calls.slice(cache.callStart).some(call=>call.method==='sendvideo' && call.cached));
    const keyword=Number(await fixtureSql("INSERT INTO nsfw_keywords(keyword,note,created_at) VALUES ('sensitivecontent','inline fixture',unixepoch('now')*1000000) RETURNING id"));
    const blur=await h.submit(selected,selection(selected)); await h.succeeded(blur.operationId);
    assert(botApi.calls.slice(blur.callStart).some(call=>call.method==='sendvideo' && call.cached && call.has_spoiler));
    const groupArticle=await lookup(direct,user,{chatType:'supergroup'}); assert.equal(groupArticle.data.results.length,1);
    const beforeDenied=botApi.sendCalls;
    const denied=await h.submit(direct,{...selection(direct),actor:user,chat:group,thread:18});
    await waitFor(async()=> (await queryJob(denied.operationId)).startsWith('failed|'),'actual topic NSFW refusal');
    assert.equal(botApi.sendCalls,beforeDenied);
    await fixtureSql(`DELETE FROM nsfw_keywords WHERE id=${keyword}`);
    const before=await operationCount();
    botApi.enqueue(user,group,direct,messageId++,'en',{...visible(direct),thread:17});
    await command('/help'); await sleep(150); assert.equal(await operationCount(),before,'disabled real topic cannot admit');

    // A selected timestamp opens the ordinary private cut dialog. Queries cannot
    // consume or modify this dialog, and cancellation is still the actual command.
    const timestamp=`http://127.0.0.1:${sourcePort}/media?case=inline-cut-dialog&t=0.2`;
    assert.equal((await lookup(timestamp)).data.results[0].input_message_content.message_text,timestamp);
    const cutStart=botApi.calls.length;
    botApi.enqueue(42,42,timestamp,messageId++,'en',visible(timestamp));
    await waitFor(()=>botApi.calls.slice(cutStart).find(call=>call.method==='sendmessage' && call.data.text.startsWith('Clip end\n')),'actual selected timestamp cut dialog');
    assert.equal(await fixtureSql("SELECT stage FROM runtime_cut_sessions WHERE actor_id=42 AND bot_id=123456 AND chat_id=42"),'end');
    const cutState=await fixtureSql("SELECT json_object('stage',stage,'revision',revision,'source',source_url,'start',start,'end',end) FROM runtime_cut_sessions WHERE actor_id=42 AND bot_id=123456 AND chat_id=42");
    await lookup('cats');
    assert.equal(await fixtureSql("SELECT json_object('stage',stage,'revision',revision,'source',source_url,'start',start,'end',end) FROM runtime_cut_sessions WHERE actor_id=42 AND bot_id=123456 AND chat_id=42"),cutState);
    await command('/cancel');
    assert.equal(await fixtureSql("SELECT stage FROM runtime_cut_sessions WHERE actor_id=42 AND bot_id=123456 AND chat_id=42"),'cancelled');

    // Owner quality chooser receives original selected URL/clip, not query state.
    await command('/settings quality ask');
    const cut=`http://127.0.0.1:${sourcePort}/media?case=inline-quality 0.2 0.5`;
    const cutArticle=await lookup(cut); assert.equal(cutArticle.data.results[0].input_message_content.message_text,cut);
    const start=botApi.calls.length; const cutMessage=messageId++;
    botApi.enqueue(42,42,cut,cutMessage,'en',visible(cut));
    const chooser=await waitFor(()=>botApi.calls.slice(start).find(call=>call.method==='sendmessage' && call.data.reply_markup),'selected clip quality chooser');
    const keyboard=typeof chooser.data.reply_markup==='string'?JSON.parse(chooser.data.reply_markup):chooser.data.reply_markup;
    const best=keyboard.inline_keyboard.flat().find(button=>button.callback_data?.startsWith('ql:best:')); assert(best);
    const saved=JSON.parse(await fixtureSql("SELECT request_json FROM runtime_quality_sessions WHERE actor_id=42 AND bot_id=123456 AND chat_id=42"));
    assert.equal(saved.clip_range.start_seconds,0.2);
    assert.equal(saved.clip_range.end_seconds,0.7);
    const queryState=await counts(); await lookup('cats'); assert.equal(await counts(),queryState,'lookup cannot edit a pending quality/cut chooser');
    botApi.setExpectedUpload({ method: 'sendvideo', filename: 'media.mp4', mimeType: 'video/mp4', preserveMdat: false });
    botApi.enqueueCallback(42,42,chooser.messageId,best.callback_data,{id:'inline-quality-best'});
    const { operationId: operation } = await admissionNotice(context, start);
    await h.succeeded(operation);
    await h.inspectUpload('inline selected quality clip', { video: true, audio: true, durationMin: 0.35, durationMax: 0.8, height: 120 });
    botApi.setExpectedUpload(null);
    await command('/settings quality best');

    const shutdownState=await counts();
    const shutdownRelease=inlineSource.hold('shutdown');
    const shutdownId=botApi.enqueueInline(42,'shutdown',{id:`native-inline-${++queryId}`});
    await waitFor(()=>inlineSource.trace.some(entry=>entry.query==='shutdown'),'shutdown search entered');
    await restartInlineAppGracefully(); shutdownRelease();
    assert.equal(await counts(),shutdownState,'graceful cancellation cannot mutate immutable query/admission state');
    assert(!botApi.calls.some(call=>call.method==='answerinlinequery' && call.data.inline_query_id===shutdownId));
    await assertWorkspaceClean();
  } finally { inlineSource.releaseAll(); botApi.setExpectedUpload(null); botApi.setExpectedAlbum(null); }
  return 'real pinned inline metadata, current grants and revocation, responsive supersession/timeout/shutdown, no query mutation, actual audio/image/replay/cache/NSFW/group-topic/owner quality-clip selections';
}
