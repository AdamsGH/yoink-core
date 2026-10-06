import test from 'node:test';
import assert from 'node:assert/strict';
import { BotApiFixture } from './bot-api.mjs';
import { InlineSource, searchMetadata, playerMetadata } from './inline-source.mjs';

test('native YouTube HTTP fixtures expose metadata and gated search, not canned cards', async () => {
  const source = new InlineSource({ mediaUrl: 'http://127.0.0.1:1234/media?case=inline', mediaBytes: Buffer.alloc(1000) });
  const release = source.hold('held');
  const response = source.response('www.youtube.com','/youtubei/v1/search',{ method:'POST', body: JSON.stringify({query:'held'}),headers:{} });
  assert.equal(response.status,200); assert(response.gate);
  const parsed = JSON.parse(response.body);
  const renderers = parsed.contents.twoColumnSearchResultsRenderer.primaryContents.sectionListRenderer.contents[0].itemSectionRenderer.contents;
  assert.equal(renderers.length,8); assert.equal(renderers[0].videoRenderer.videoId,'fixture0000');
  assert(!Object.hasOwn(parsed,'results'),'fixture must exercise actual installed metadata parsing');
  release(); await response.gate;
  assert.equal(source.trace.length,1);
  assert.equal(JSON.parse(source.response('www.youtube.com','/youtubei/v1/player',{body:JSON.stringify({videoId:'fixture0000'})}).body).streamingData.formats[0].contentLength,'1000');
  assert.equal(source.response('www.youtube.com','/youtubei/v1/search',{body:JSON.stringify({query:'network denial'})}).headers.location,'http://127.0.0.2:1/private-secret');
  assert.equal(searchMetadata('zero').contents.twoColumnSearchResultsRenderer.primaryContents.sectionListRenderer.contents[0].itemSectionRenderer.contents.length,0);
  assert.equal(playerMetadata('fixture0000','http://source/media',1000).videoDetails.videoId,'fixture0000');
  assert.throws(()=>source.response('www.youtube.com','/youtubei/v1/search',{headers:{cookie:'auth=secret'},body:'{}'}));
});

test('raw inline Bot API fixture records personal zero-cache text cards and real selection routes', async t => {
  const api = new BotApiFixture({ mediaBytes: Buffer.alloc(16) });
  const port = await api.start(); t.after(()=>api.close());
  const post = async (method,data) => {
    const response = await fetch(`http://127.0.0.1:${port}/botfixture/${method}`,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify(data)});
    return { status:response.status, body:await response.json() };
  };
  const id = api.enqueueInline(43,'cats',{chatType:'supergroup',language:'ru'});
  const update = (await post('getupdates',{allowed_updates:['inline_query']})).body.result[0];
  assert.equal(update.inline_query.from.id,43); assert.equal(update.inline_query.chat_type,'supergroup');
  const url='https://www.youtube.com/watch?v=fixture0000';
  const result={type:'article',id:'stable',title:'Fixture',input_message_content:{message_text:url,link_preview_options:{is_disabled:true}}};
  assert.equal((await post('answerinlinequery',{inline_query_id:id,is_personal:true,cache_time:0,results:[result]})).body.result,true);
  api.setInlineMode('failure-once'); assert.equal((await post('answerinlinequery',{inline_query_id:'failed',is_personal:true,cache_time:0,results:[]})).status,500);
  api.enqueue(43,-77,url,10,'ru',{viaBot:123456,thread:18,entities:[{type:'url',offset:0,length:url.length}]});
  const selected=(await post('getupdates',{offset:update.update_id+1,allowed_updates:['message']})).body.result[0].message;
  assert.equal(selected.chat.id,-77); assert.equal(selected.message_thread_id,18); assert.equal(selected.via_bot.id,123456);
  assert.equal(selected.text,url);
});
