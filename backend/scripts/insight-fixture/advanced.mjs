import assert from 'node:assert/strict';
import { ARTICLE } from './services.mjs';
import { configure, session } from './scenarios.mjs';
export const BYOK_BASE='https://byok.fixture.invalid/v1';
export const byokBody={provider:'custom_openai',base_url:BYOK_BASE,model:'fixture-model'};
// Explicit isolated imported readiness is useful for testing invalidation, not
// evidence of a successful public-TLS probe. Never label this seed a probe pass.
export async function seedByok(c,uid,{ready=false,key='fixture-byok-one'}={}) {
  assert([42,43,44,45].includes(uid));assert(['fixture-byok-one','fixture-byok-two'].includes(key));
  await c.db.batch(`INSERT INTO insight_user_byok(user_id,provider,base_url,api_key,model,models_json,tested_at,test_error,created_at,updated_at) VALUES(${uid},'custom_openai','${BYOK_BASE}','${key}','fixture-model','[{"id":"fixture-model","supports_websearch":false}]',${ready?'unixepoch()*1000000':'NULL'},${ready?'NULL':"'provider_probe_failed'"},0,unixepoch()*1000000) ON CONFLICT(user_id) DO UPDATE SET provider=excluded.provider,base_url=excluded.base_url,api_key=excluded.api_key,model=excluded.model,models_json=excluded.models_json,tested_at=excluded.tested_at,test_error=excluded.test_error,updated_at=excluded.updated_at;`);
}
const names=(c,uid)=>c.bot.commandMenus.privateCommands(uid,'en').map(x=>x.command);
async function menu(c,uid,name,present){await c.waitFor(()=>names(c,uid).includes(name)===present,`fresh ${name} menu ${uid} ${present}`);}
export async function byok(c) {
  const {request:owner}=await configure(c),user=await session(c,45);
  await owner('/config/byok',{method:'PATCH',body:{enabled:true}});
  try {
    const saved=await user('/byok/me',{method:'PUT',body:{...byokBody,api_key:'fixture-byok-one'}});
    assert.equal(saved.api_key_set,true);assert.equal(saved.test_error,'provider_probe_failed');assert.equal(saved.tested_at,null);
    assert.equal((await user('/settings/me')).has_tldr_access,false,'failed real native probe cannot grant access');
    const omitted=await user('/byok/me/test',{method:'POST',body:{provider:byokBody.provider,base_url:BYOK_BASE}});
    assert.equal(omitted.ok,false);assert.equal(omitted.error,'provider_probe_failed','omitted key reaches stored-key safety/probe path');
    const empty=await user('/byok/me/test',{method:'POST',body:{provider:byokBody.provider,base_url:BYOK_BASE,api_key:''}});
    assert.equal(empty.ok,false);assert.equal(empty.error,'api_key_required');
    await user('/byok/me',{method:'PUT',body:byokBody});
    assert.equal(await c.db.query("SELECT api_key='fixture-byok-one' FROM insight_user_byok WHERE user_id=45"),'1','PUT omission preserves exact stored key');
    await owner('/config/byok',{method:'PATCH',body:{enabled:false}});
    await user('/byok/me/test',{method:'POST',body:{provider:byokBody.provider},expected:403});
    // Imported tested state is allowed as fixture DB mutation. Toggle must use
    // current state, including menu refresh; it must not permanently latch grant.
    await seedByok(c,45,{ready:true});
    c.bot.enqueue(45,45,'/start',32000,'en');
    await c.waitFor(()=>c.bot.commandMenus.has({scope:{type:'chat',chat_id:45},language_code:''}),'real prepared private menu');
    assert.equal((await user('/settings/me')).has_tldr_access,false);
    await owner('/config/byok',{method:'PATCH',body:{enabled:true}});
    assert.equal((await user('/settings/me')).has_tldr_access,true);await menu(c,45,'tldr',true);
    await user('/byok/me',{method:'PUT',body:{...byokBody,api_key:''}});
    assert.equal((await user('/byok/me')).api_key_set,false);assert.equal((await user('/settings/me')).has_tldr_access,false);await menu(c,45,'tldr',false);
    await seedByok(c,45,{ready:true});
    await owner('/config/byok',{method:'PATCH',body:{enabled:true}});await menu(c,45,'tldr',true);
    await user('/byok/me',{method:'DELETE',expected:204});await menu(c,45,'tldr',false);
    // Public access mutation grants SUMMARY, not TLDR. Keep the distinction.
    await owner('/access/45',{method:'POST',body:{lang:'en'},expected:201});
    assert.equal((await user('/settings/me')).has_gemini_access,true);await menu(c,45,'summary',true);
    await owner('/access/45',{method:'DELETE',expected:204});
    assert.equal((await user('/settings/me')).has_gemini_access,false);await menu(c,45,'summary',false);
  } finally {await c.db.batch('DELETE FROM insight_user_byok WHERE user_id=45');await owner('/config/byok',{method:'PATCH',body:{enabled:false}});}
}
export async function policy(c) {
  const {request:owner,body}=await configure(c),user=await session(c,43);
  const invalid=await user('/settings/me',{method:'PATCH',body:{lang:'en',tldr_model:'not-allowed'},expected:400});assert.equal(invalid.detail,'model_not_allowed');
  await user('/settings/me',{method:'PATCH',body:{lang:'en',tldr_model:'fixture-model'}});
  const wrong=await owner('/config/tldr',{method:'PATCH',body:{...body,default_model:'not-allowed'},expected:400});assert.equal(wrong.detail,'invalid_request');
  const models=await user('/models');assert.deepEqual(models.map(x=>x.id),['fixture-model']);
  await user('/config/tldr',{expected:403});
}
export async function cacheIsolation(c) {
  const {request:owner,body}=await configure(c),user=await session(c,43);
  const query={command:'tldr',url:ARTICLE};
  await user('/settings/me',{method:'PATCH',body:{lang:'en',prompts:{tldr:'Isolate cache fixture.'},github_token:'fixture-oauth-read'}});
  const first=await user('/summarize',{method:'POST',body:query});assert.equal(first.cached,false);
  assert.equal((await user('/summarize',{method:'POST',body:query})).cached,true);
  try {
    await owner('/config/tldr',{method:'PATCH',body:{...body,gateway_api_key:'fixture-gateway-two'}});
    const changed=await user('/summarize',{method:'POST',body:query});assert.equal(changed.cached,false,'fresh provider credential must partition private cache');
    assert.equal(c.trusted.calls.filter(x=>x.kind==='completion').at(-1).credential,'two');
    await user('/settings/me',{method:'PATCH',body:{lang:'en',github_token:''}});
    assert.equal((await user('/summarize',{method:'POST',body:query})).cached,false,'source token clearing partitions private cache');
  } finally {await owner('/config/tldr',{method:'PATCH',body});await user('/settings/me',{method:'PATCH',body:{lang:'en',prompts:{tldr:null},github_token:''}});}
  await c.db.batch("INSERT INTO insight_summary_cache(content_key,lang,command,result,expires_at) VALUES('dQw4w9WgXc','en','summary','UNVERIFIABLE PRIVATE IMPORT',(unixepoch()+600)*1000000),('http://93.184.216.34/article','en','tldr','UNVERIFIABLE PRIVATE IMPORT',(unixepoch()+600)*1000000);");
  const snapshot=await c.db.query("SELECT id,content_key,lang,command,result,expires_at FROM insight_summary_cache WHERE result='UNVERIFIABLE PRIVATE IMPORT' ORDER BY id");
  await c.db.batch('DELETE FROM insight_native_cache WHERE user_id=43'); // Not the imported/quarantined table.
  const contentBefore=c.trusted.calls.filter(x=>x.kind==='content').length;
  const actual=await user('/summarize',{method:'POST',body:query});
  assert.equal(c.trusted.calls.filter(x=>x.kind==='content').length,contentBefore+1,'quarantined import cannot short-circuit actual worker');assert.equal(actual.cached,false);assert(!actual.result.includes('UNVERIFIABLE'));
  assert.equal(await c.db.query("SELECT id,content_key,lang,command,result,expires_at FROM insight_summary_cache WHERE result='UNVERIFIABLE PRIVATE IMPORT' ORDER BY id"),snapshot,'quarantine never deletes imported rows');
}
export async function failures(c) {
  await configure(c);const user=await session(c,43);
  for(const kind of ['content','provider']) {
    const flag=kind==='content'?'contentFailure':'providerFailure';c.trusted[flag]=true;
    try {
      const error=await user('/summarize',{method:'POST',body:{command:'tldr',url:ARTICLE,focus:`failure-${kind}`},expected:kind==='content'?400:502});
      // Source failure enters anonymous fallbacks, denied by the confined proxy.
      assert.equal(error.detail,kind==='content'?'content_network_denied':'provider_unavailable');
      await c.waitFor(async()=>await c.db.query('SELECT count(*) FROM insight_native_reservations WHERE user_id=43')==='0',`${kind} failure releases quota`);
    } finally {c.trusted[flag]=false;}
    assert.equal((await user('/summarize',{method:'POST',body:{command:'tldr',url:ARTICLE,focus:`recovered-${kind}`}})).cached,false,'failure leaves capacity for next real request');
  }
}
export async function cancellation(c) {
  await configure(c);const token=await c.login(43),controller=new AbortController();c.trusted.holdProvider=true;
  const pending=fetch(`http://127.0.0.1:${c.httpPort}/api/v1/insight/summarize`,{method:'POST',headers:{authorization:`Bearer ${token}`,'content-type':'application/json'},body:JSON.stringify({command:'tldr',url:ARTICLE,focus:'http-disconnect'}),signal:controller.signal}).then(r=>({status:r.status}),error=>({error}));
  try {
    await c.waitFor(()=>c.trusted.providers.size===1,'HTTP client has actual held native completion');controller.abort();assert((await pending).error,'abort is not a successful response');
    await c.waitFor(async()=>await c.db.query('SELECT count(*) FROM insight_native_reservations WHERE user_id=43')==='0','HTTP disconnect cancels admission',5000);
    await c.waitFor(()=>c.trusted.providers.size===0,'native disconnect closes provider',5000);
  } finally {controller.abort();c.trusted.holdProvider=false;c.trusted.releaseProviders();await pending;}
  const start=c.bot.calls.length;c.trusted.holdStream=true;c.bot.enqueue(44,44,`/tldr ${ARTICLE} cancellation-work`,33001,'en');
  try {
    const draft=await c.waitFor(()=>c.trusted.providers.size===1 && c.bot.calls.slice(start).find(x=>Number(x.data.chat_id)===44 && /\/cancel [a-f0-9-]{36}/.test(x.data.text??'')),'actual streaming draft publishes cancel ID');
    const id=draft.data.text.match(/\/cancel ([a-f0-9-]{36})/)[1];assert(c.trusted.providers.size>0);
    // Wrong actor cannot cancel another user's admitted request.
    c.bot.enqueue(43,43,`/cancel ${id}`,33002,'en');
    c.bot.enqueue(44,44,`/cancel ${id}`,33003,'en');
    await c.waitFor(async()=>await c.db.query('SELECT count(*) FROM insight_native_reservations WHERE user_id=44')==='0','TG cancellation releases quota');
    await c.waitFor(()=>c.trusted.providers.size===0,'TG cancellation closes streamed provider');
    assert(!c.bot.calls.slice(start).some(x=>Number(x.data.chat_id)===44 && String(x.data.text).includes('Native café summary.')),'cancelled draft never becomes a final result');
  } finally {c.trusted.holdStream=false;c.trusted.releaseProviders();}
  const user=await session(c,44);assert.equal((await user('/summarize',{method:'POST',body:{command:'tldr',url:ARTICLE,focus:'after-cancel'}})).cached,false);
}
export async function heldRevocation(c) {
  const {request:owner,body}=await configure(c);const user=await session(c,43);
  for(const kind of ['grant','token']) {
    c.trusted.holdProvider=true;
    const pending=user('/summarize',{method:'POST',body:{command:'tldr',url:ARTICLE,focus:`held-${kind}`},expected:kind==='grant'?403:400}).then(value=>({value}),error=>({error}));
    try {
      await c.waitFor(()=>c.trusted.providers.size===1,`live held ${kind} revocation`);
      if(kind==='grant')await c.db.batch("DELETE FROM user_permissions WHERE user_id=43 AND plugin='insight' AND feature='tldr'");
      else await owner('/config/tldr',{method:'PATCH',body:{...body,gateway_api_key:''}});
      c.trusted.releaseProviders();const outcome=await pending;if(outcome.error)throw outcome.error;
      assert.equal(outcome.value.detail,kind==='grant'?'access_denied':'route_changed');
      await c.waitFor(async()=>await c.db.query('SELECT count(*) FROM insight_native_reservations WHERE user_id=43')==='0',`${kind} revocation releases quota`);
    } finally {c.trusted.holdProvider=false;c.trusted.releaseProviders();await pending;await c.db.batch("INSERT INTO user_permissions(user_id,plugin,feature,granted_by,granted_at) VALUES(43,'insight','tldr',42,0) ON CONFLICT(user_id,plugin,feature) DO NOTHING");await owner('/config/tldr',{method:'PATCH',body});}
  }
}

export async function heldDns(c) {
  await c.stop();const dns=await c.startDns();let pending;
  try {
    await c.launch({dns:dns.address});await c.ready();const {request}=await configure(c);
    pending=request('/summarize',{method:'POST',body:{command:'tldr',url:`https://${dns.hostname}/held`,focus:'held DNS shutdown'}}).then(value=>({value}),error=>({error}));
    const worker=await c.waitFor(async()=>{
      const state=await dns.state();if(!state.records.some(x=>x.name===dns.hostname&&x.held&&!x.replied))return false;
      const snapshot=await c.observeDnsWorker();assert.deepEqual(snapshot.errors,[],'read-only proc evidence, never widen capabilities');
      assert.match(snapshot.resolver,/nameserver 127\.0\.0\.11/,'owned internal bridge uses Docker stub forwarding to the confined DNS service');
      const child=snapshot.workers.find(x=>snapshot.children.includes(x.pid)&&x.sockets.length>0);
      if(!child)return false;
      assert.deepEqual(child.argv,['/usr/local/bin/python','-I','-B','/opt/yoink-worker/main.py']);assert.equal(child.parent,1);
      assert(child.cwd.startsWith('/state/insight/'));assert.equal(child.mode,0o700);assert.deepEqual(child.uid,Array(4).fill('10001'));assert.equal(child.noNewPrivs,'1');assert(/^0+$/.test(child.capEff));assert.deepEqual(JSON.parse(child.privateOrigins),[]);
      assert.deepEqual(JSON.parse(child.proxyOrigins),[c.trusted.origin]);return child;
    },'actual confined worker owns a Docker DNS-stub socket while exact DNS question remains held',10000);
    const boundary=await c.observeDnsWorker();assert(boundary.workers.some(x=>x.pid===worker.pid&&x.sockets.length>0));assert.equal((await dns.state()).phase,'holding');
    await c.stop({live:true});assert.equal((await dns.state()).phase,'holding','DNS was not released to help original child exit');
    assert.equal(await c.db.query('SELECT count(*) FROM insight_native_reservations'),'0');await c.workspaceClean();
    const result=await pending;assert(!result.value,'held DNS request cannot publish a result at shutdown');
  } finally {await c.stop();await dns.release();await dns.close();if(pending)await pending;}
  await c.launch();await c.ready();
}

export async function telegramRevocation(c) {
  await configure(c);const before=c.bot.calls.length;c.trusted.holdStream=true;
  c.bot.enqueue(44,44,`/tldr ${ARTICLE} tg-held-revoke`,34001,'en');
  try {
    await c.waitFor(()=>c.trusted.providers.size===1&&c.bot.calls.slice(before).some(x=>Number(x.data.chat_id)===44&&String(x.data.text).includes('Draft only.')),'real TG provider stream held after incomplete draft');
    assert.equal(await c.db.query('SELECT count(*) FROM insight_native_reservations WHERE user_id=44'),'1');
    await c.db.batch("DELETE FROM user_permissions WHERE user_id=44 AND plugin='insight' AND feature='tldr'");
    c.trusted.releaseProviders();
    await c.waitFor(async()=>await c.db.query('SELECT count(*) FROM insight_native_reservations WHERE user_id=44')==='0','fresh TG grant revoke releases quota');
    await c.waitFor(()=>c.trusted.providers.size===0,'revoked TG provider closes');
    assert(!c.bot.calls.slice(before).some(x=>Number(x.data.chat_id)===44&&String(x.data.text).includes('Native café summary.')),'revoked stream cannot publish final completion');
  } finally {
    c.trusted.holdStream=false;c.trusted.releaseProviders();
    await c.db.batch("INSERT INTO user_permissions(user_id,plugin,feature,granted_by,granted_at) VALUES(44,'insight','tldr',42,0) ON CONFLICT(user_id,plugin,feature) DO NOTHING");
  }
  const user=await session(c,44);assert.equal((await user('/summarize',{method:'POST',body:{command:'tldr',url:ARTICLE,focus:'after-tg-revoke'}})).cached,false);
}
// Observe both the immediate abandoned-future cleanup and the original-process
// shutdown join. Never infer detachment from the implementation alone.
export async function abortedQuota(c) {
  await configure(c);const token=await c.login(43),user=await session(c,43);
  assert.equal(await c.db.query('SELECT count(*) FROM insight_native_reservations WHERE user_id=43'),'0','no existing reservations are hidden by setup');
  const usageSql="SELECT * FROM insight_usage_log WHERE user_id=43 AND status='ok' ORDER BY id",used=await c.db.query(usageSql);
  let controller=new AbortController();c.trusted.holdProvider=true;
  const startRequest=focus=>fetch(`http://127.0.0.1:${c.httpPort}/api/v1/insight/summarize`,{method:'POST',headers:{authorization:`Bearer ${token}`,'content-type':'application/json'},body:JSON.stringify({command:'tldr',url:ARTICLE,focus}),signal:controller.signal}).then(r=>({status:r.status}),error=>({error}));
  let pending=startRequest('quota-drop-observation');
  try {
    await c.waitFor(()=>c.trusted.providers.size===1,'quota request waiting in real provider');
    assert.equal(await c.db.query('SELECT count(*) FROM insight_native_reservations WHERE user_id=43'),'1','observe real quota reservation before abort');
    controller.abort();assert((await pending).error);
    await c.waitFor(async()=>await c.db.query('SELECT count(*) FROM insight_native_reservations WHERE user_id=43')==='0','aborted HTTP reservation cleanup observed',5000);
    assert.equal(await c.db.query(usageSql),used,'aborted reservation does not charge usage');
    await c.waitFor(()=>c.trusted.providers.size===0,'first aborted request closes provider');
    controller=new AbortController();pending=startRequest('quota-drop-at-shutdown');
    await c.waitFor(()=>c.trusted.providers.size===1,'second real provider held at shutdown race');
    assert.equal(await c.db.query('SELECT count(*) FROM insight_native_reservations WHERE user_id=43'),'1');
    controller.abort();assert((await pending).error);
    // No cleanup wait/barrier here: exercise the shared shutdown join itself.
    await c.stop();
    assert.equal(await c.db.query('SELECT count(*) FROM insight_native_reservations WHERE user_id=43'),'0','original process drained quota cleanup before DB close');
    assert.equal(await c.db.query(usageSql),used,'no late cleanup charge after shutdown');
  } finally {controller.abort();c.trusted.holdProvider=false;c.trusted.releaseProviders();await pending;}
  await c.launch();await c.ready();assert.equal((await user('/summarize',{method:'POST',body:{command:'tldr',url:ARTICLE,focus:'quota-after-drop'}})).cached,false);
}
export async function receiptShutdown(c) {
  await configure(c);const message=39001,base=`insight:123456:44:0:${message}:0:0`,before=c.bot.calls.length;
  const finalToken=`insight:123456:44:0:${message}:0:1`;
  c.trusted.holdReceipt=true;c.bot.enqueue(44,44,`/tldr ${ARTICLE} receipt-shutdown`,message,'en');
  try {
    await c.waitFor(()=>c.trusted.receipts.size===1,'actual Bot API receipt stalled');
    assert.equal(await c.db.query(`SELECT state='sending' AND receipt IS NULL FROM runtime_text_effects WHERE token='${finalToken}'`),'1','durable final admission precedes stalled receipt');
    assert.equal(await c.db.query(`SELECT state FROM runtime_text_effects WHERE token='${base}'`),'sealed','preview has no durable receipt');
    await c.stop();
    assert.equal(c.trusted.receipts.size,0,'original-process shutdown closes stalled receipt socket');
    const stopped=await c.db.query(`SELECT state,receipt,bound FROM runtime_text_effects WHERE token='${finalToken}'`);
    assert.equal(await c.db.query(`SELECT state IN ('sending','unknown') AND receipt IS NULL FROM runtime_text_effects WHERE token='${finalToken}'`),'1','receipt never confirms after cancellation');
    const count=c.bot.calls.length;c.trusted.releaseReceipts();
    assert.equal(await c.db.query(`SELECT state,receipt,bound FROM runtime_text_effects WHERE token='${finalToken}'`),stopped,'late receipt cannot mutate closed-store effect');
    assert.equal(c.bot.calls.length,count,'no handler work after original process exit');
    c.trusted.holdReceipt=false;await c.launch();await c.ready();
    assert.equal((await c.replayStatus(base)).fenced,0);
    const replay=c.bot.calls.length;c.bot.enqueue(44,44,`/tldr ${ARTICLE} receipt-shutdown`,message,'en');
    c.bot.enqueue(45,45,'/start',message+1,'en');
    await c.waitFor(()=>c.bot.calls.slice(replay).some(x=>x.method==='sendmessage' && Number(x.data.chat_id)===45),'separate live Telegram handler');
    await c.waitFor(async()=>(await c.replayStatus(base)).fenced===1,'exact stalled-receipt replay fence');
    await c.stop(); // drain replay handlers before the negative assertion
    assert.equal(c.bot.calls.slice(before).filter(x=>x.method==='sendmessage' && Number(x.data.chat_id)===44 && Number(x.data.reply_parameters?.message_id)===message).length,1,'already admitted effect is replay-fenced');
  } finally {c.trusted.holdReceipt=false;c.trusted.releaseReceipts();}
}
export async function genericReplyShutdown(c) {
  const before=c.bot.calls.length;
  c.trusted.holdReceipt=true;c.bot.enqueue(45,45,'/start',39101,'en');
  try {
    await c.waitFor(()=>c.trusted.receipts.size===1,'generic reply response is held');
    assert(c.bot.calls.slice(before).some(x=>x.method==='sendmessage'&&Number(x.data.chat_id)===45),'real generic response reached Bot API');
    await c.stop();
    assert.equal(c.trusted.receipts.size,0,'configured transport deadline closed unreleased generic response');
    const logs=await c.processLogs();
    assert.match(logs,/fixture Insight handlers drained/);
    assert(!logs.includes('shutdown deadline exceeded'),'ordinary held reply finishes inside unchanged adapter shutdown deadline');
  } finally {c.trusted.holdReceipt=false;c.trusted.releaseReceipts();}
}
