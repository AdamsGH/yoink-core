import assert from 'node:assert/strict';
import { ARTICLE } from './services.mjs';
import { initData, literal } from './helpers.mjs';
const root='/api/v1/insight';
export async function session(c,uid=42){await c.ready();const token=await c.login(uid);return (path,options={})=>c.request(root+path,{token,...options});}
export async function routing(c){
  await c.launch({insight:false});await c.ready();await c.request(root+'/settings/me',{expected:404});
  const disabled=await c.request('/api/v1/system');assert.equal(disabled.ready,true);assert.equal(disabled.modules.find(x=>x.id==='insight').status,'planned');await c.stop();
  const release=c.bot.holdNextMenu();
  const menuBefore=c.bot.calls.length;
  try {await c.launch({insight:true,inbox:false});await c.waitFor(()=>c.bot.calls.slice(menuBefore).some(x=>x.status==='hold-once'),'actual menu request is held');assert.equal((await fetch(`http://127.0.0.1:${c.httpPort}/health/ready`,{signal:AbortSignal.timeout(1000)})).status,503,'held menu request prevents readiness');}
  finally {release();}
  await c.ready();const configured=await c.request('/api/v1/system');assert.equal(configured.ready,true);assert.equal(configured.modules.find(x=>x.id==='insight').status,'experimental');
  await c.request('/api/v1/inbox/items',{expected:404});await c.request(root+'/settings/me',{expected:401});
}
export async function identity(c){
  const owner=await session(c), user=await session(c,43);
  await owner('/settings/me');await user('/settings/me');await user('/config/tldr',{expected:403});await owner('/config/tldr');
  // Login is real Telegram WebApp HMAC verification, not fixture-forged JWT.
  await c.request('/api/v1/auth/token',{method:'POST',body:{init_data:initData(42,'wrong-fixture-token')},expected:401});
  await c.db.batch("UPDATE users SET role='banned' WHERE id=43");
  try {await user('/settings/me',{expected:403});} finally {await c.db.batch("UPDATE users SET role='user' WHERE id=43");}
  const token=await c.login(42);
  for (const scopes of [['*'], ['users:read']]) {
    const key=await c.request('/api/v1/api-keys',{token,method:'POST',body:{name:'Insight fixture',scopes},expected:201});
    c.addSecret(key.key);
    const headers={'X-Api-Key':key.key};
    try {
      await c.request('/api/internal/v1/status',{headers,expected:scopes[0]==='*'?200:403});
      await c.request(root+'/settings/me',{headers,expected:401});
      await c.request(root+'/config/tldr',{headers,expected:401});
      await c.request(root+'/settings/me',{token,headers});
      await c.request(root+'/settings/me',{token:'invalid-session',headers,expected:401});
    } finally {await c.request(`/api/v1/api-keys/${key.id}`,{token,method:'DELETE',expected:204});}
    await c.request('/api/internal/v1/status',{headers,expected:401});
  }
}
export async function configure(c){
  const request=await session(c);
  const body={allowed_models:['fixture-model'],default_model:'fixture-model',gateway_base_url:c.trusted.origin+'/v1',ai_route_mode:'a2a',gateway_api_key:'fixture-gateway'};
  await request('/config/tldr',{method:'PATCH',body});return {request,body};
}
export async function management(c){
  const {request,body}=await configure(c);
  try {
    const probes=c.trusted.calls.filter(x=>x.kind==='models').length;
    assert.equal((await request('/config/test')).ok,true,'omitted gateway key probe uses saved credential');
    assert.equal(c.trusted.calls.filter(x=>x.kind==='models').length,probes+1);
    await request('/config/tldr',{method:'PATCH',body:{...body,gateway_api_key:'fixture-management-key'}});
    let view=await request('/config/tldr');assert.equal(view.gateway_api_key,'');assert.equal(view.gateway_api_key_set,true);
    const {gateway_api_key:ignored,...omitted}=body;
    await request('/config/tldr',{method:'PATCH',body:omitted});assert.equal((await request('/config/tldr')).gateway_api_key_set,true);
    assert.equal(await c.db.query("SELECT value='fixture-management-key' FROM bot_settings WHERE key='insight_tldr_gateway_key'"),'1','omission preserves exact stored credential without disclosing it');
    await request('/config/tldr',{method:'PATCH',body:{...omitted,gateway_api_key:''}});assert.equal((await request('/config/tldr')).gateway_api_key_set,false,'explicit empty must clear, unlike omission');
    const alias=await request('/aliases',{method:'POST',body:{aliases:'fixture_alias',prompt:'Use concise prose.'},expected:201});
    assert(Number.isSafeInteger(alias.id));
    try {await request(`/aliases/${alias.id}`,{method:'PATCH',body:{aliases:'fixture_alias',prompt:'Use clear concise prose.'}});assert((await request('/aliases')).some(x=>x.id===alias.id));}
    finally {await request(`/aliases/${alias.id}`,{method:'DELETE',expected:204});}
  } finally {await request('/config/tldr',{method:'PATCH',body});}
}
export async function summary(c){
  const {request}=await configure(c);const body={command:'tldr',url:ARTICLE};
  const before=c.trusted.calls.filter(x=>x.kind==='completion').length;
  const first=await request('/summarize',{method:'POST',body});
  assert.equal(first.result,'Native café summary.');assert.equal(first.cached,false);assert.equal(typeof first.route,'string');assert(Object.hasOwn(first,'alias'));
  assert.equal(c.trusted.calls.filter(x=>x.kind==='completion').length,before+1);assert(c.trusted.calls.some(x=>x.kind==='content'));
  const replay=await request('/summarize',{method:'POST',body});assert.equal(replay.cached,true);assert.equal(replay.result,first.result);
  assert.equal(c.trusted.calls.filter(x=>x.kind==='completion').length,before+1);
  // TL;DR admission precedes native replay. Do not confuse summary ordering.
  const contentBefore=c.trusted.calls.filter(x=>x.kind==='content').length;
  await c.db.batch(`INSERT INTO insight_native_reservations(id,user_id,command,created_at,expires_at) VALUES ${Array.from({length:20},(_,i)=>`('fixture-quota-${i}',42,'tldr',unixepoch()*1000000,(unixepoch()+600)*1000000)`).join(',')};`);
  try {
    const denied=await request('/summarize',{method:'POST',body,expected:429});assert.equal(denied.detail,'quota_exceeded');
    assert.equal(c.trusted.calls.filter(x=>x.kind==='completion').length,before+1);
    assert.equal(c.trusted.calls.filter(x=>x.kind==='content').length,contentBefore,'TLDR quota rejection precedes extraction/cache');
  } finally {await c.db.batch("DELETE FROM insight_native_reservations WHERE id LIKE 'fixture-quota-%'");}
  const privateRequest=await session(c,43);
  const isolated=await privateRequest('/summarize',{method:'POST',body});assert.equal(isolated.cached,false,'another identity cannot reuse owner cache');
  assert.equal(c.trusted.calls.filter(x=>x.kind==='completion').length,before+2);
}
export async function telegram(c){
  await configure(c);const start=c.bot.calls.length,providers=c.trusted.calls.length;
  // Focus deliberately bypasses cache, including the earlier forced-receipt prime.
  c.bot.enqueue(44,44,`/tldr ${ARTICLE} native-tg-stream`,31001,'en');
  const final=await c.waitFor(()=>c.bot.calls.slice(start).find(x=>['sendmessage','editmessagetext'].includes(x.method) && Number(x.data.chat_id)===44 && /^Complete · tldr\n\/cancel [0-9a-f-]+\n\nNative café summary\.$/.test(String(x.data.text))),'native Telegram completed summary');
  assert.notEqual(Number(final.data.message_id??final.messageId),31001,'bot response is not the user source message');
  await c.waitFor(async()=>await c.db.query(`SELECT count(*) FROM runtime_text_effects WHERE token LIKE 'insight:123456:44:0:31001:0:%' AND state='confirmed' AND receipt=${Number(final.data.message_id??final.messageId)} AND json_extract(identity,'$.text')=${literal(final.data.text)}`)==='1','completed text has its exact durable confirmed receipt');
  assert(c.trusted.calls.slice(providers).some(x=>x.kind==='content'),'native TG uses actual full-content worker');
  assert(c.trusted.calls.slice(providers).some(x=>x.kind==='completion' && x.stream),'native Telegram adapter consumes fragmented SSE/UTF-8');
  assert.equal(c.bot.error,undefined);
}
export async function revocation(c){
  await configure(c);const request=await session(c,43);
  c.trusted.holdProvider=true;
  const pending=request('/summarize',{method:'POST',body:{command:'tldr',url:ARTICLE,focus:'fresh-ban-held-provider'},expected:403});
  // Attach immediately: request failure must not escape as an unhandled rejection
  // while the native fixture controller changes the independent user state.
  const outcome=pending.then(value=>({value}),error=>({error}));
  try {
    await c.waitFor(()=>c.trusted.providers.size>0,'native completion held after quota admission',5000);
    assert.equal(await c.db.query('SELECT count(*) FROM insight_native_reservations WHERE user_id=43'),'1');
    await c.db.batch("UPDATE users SET role='banned' WHERE id=43");
    c.trusted.releaseProviders();const result=await outcome;if(result.error)throw result.error;
    assert.equal(await c.db.query('SELECT count(*) FROM insight_native_reservations WHERE user_id=43'),'0','fresh ban rejection releases quota');
  } finally {
    c.trusted.holdProvider=false;c.trusted.releaseProviders();await outcome;
    await c.db.batch("UPDATE users SET role='user' WHERE id=43");
  }
}
export async function shutdown(c){
  const {holdShutdownOAuth,checkShutdownOAuth}=await import('./oauth.mjs');
  await holdShutdownOAuth(c);
  const {request}=await configure(c);c.trusted.holdContent=true;
  // API helper has a bounded timeout, but worker must be alive at ORIGINAL TERM.
  const pending=request('/summarize',{method:'POST',body:{command:'tldr',url:ARTICLE,focus:'held shutdown'}}).catch(()=>null);
  try {await c.waitFor(()=>c.trusted.held.size>0,'actual content worker reached declared operator proxy',5000);await c.stop({live:true});}
  finally {c.trusted.holdContent=false;c.trusted.releasePolls();await pending;}
  await checkShutdownOAuth(c);
  assert.equal(await c.db.query('SELECT count(*) FROM insight_native_reservations'),'0','shutdown releases reservations');
  await c.launch();await c.ready();
}
