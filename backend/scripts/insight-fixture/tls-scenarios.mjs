import assert from 'node:assert/strict';
import { configure, session } from './scenarios.mjs';
import { VIDEO_URL, TLS_RESULT } from './youtube-responses.mjs';
export const SUCCESS_BASE='https://byok-success.fixture.invalid/v1';
export const successByokBody={provider:'custom_openai',base_url:SUCCESS_BASE,model:'fixture-model'};
const count=async(c,kind)=>(await c.tls.state()).events.filter(e=>e.kind===kind).length;
// Coordinator launches with topology mounts/native trust roots and direct source
// transport before these gates. No seed of tested_at is used here.
export async function positiveByok(c){
 const {request:owner}=await configure(c),user=await session(c,45);
 await owner('/config/byok',{method:'PATCH',body:{enabled:true}});
 try{
  const before=await count(c,'models');
  const saved=await user('/byok/me',{method:'PUT',body:{...successByokBody,api_key:'fixture-byok-one'}});
  assert.equal(saved.api_key_set,true);assert.equal(saved.test_error,null);assert(saved.tested_at);
  assert.equal((await user('/settings/me')).has_tldr_access,true);
  const probe=await user('/byok/me/test',{method:'POST',body:{provider:successByokBody.provider,base_url:SUCCESS_BASE}});assert.equal(probe.ok,true);
  await user('/byok/me',{method:'PUT',body:successByokBody});assert.equal((await user('/byok/me')).test_error,null);
  assert((await count(c,'models'))>=before+3,'PUT and omitted-key paths reached authenticated TLS catalogue');
 }finally{await user('/byok/me',{method:'DELETE',expected:204});await owner('/config/byok',{method:'PATCH',body:{enabled:false}});}
}
export async function positiveCacheIsolation(c){
 const {request:owner}=await configure(c),user=await session(c,43);
 const body={command:'tldr',url:'https://byok-success.fixture.invalid/article'};
 await owner('/config/byok',{method:'PATCH',body:{enabled:true}});
 try{
  await user('/byok/me',{method:'PUT',body:{...successByokBody,api_key:'fixture-byok-one'}});
  const before=await count(c,'byok-completion');
  const first=await user('/summarize',{method:'POST',body});assert.equal(first.cached,false);assert.equal(first.result,TLS_RESULT);
  assert.equal((await user('/summarize',{method:'POST',body})).cached,true);assert.equal(await count(c,'byok-completion'),before+1);
  await user('/byok/me',{method:'DELETE',expected:204});
  const gatewayBefore=c.trusted.calls.filter(e=>e.kind==='completion').length;
  const gateway=await user('/summarize',{method:'POST',body});assert.equal(gateway.cached,false);assert.equal(gateway.result,'Native café summary.');
  assert.equal(c.trusted.calls.filter(e=>e.kind==='completion').length,gatewayBefore+1);
  assert.equal((await user('/summarize',{method:'POST',body})).cached,true);
  await user('/byok/me',{method:'PUT',body:{...successByokBody,api_key:'fixture-byok-one'}});
  const returned=await user('/summarize',{method:'POST',body});assert.equal(returned.result,TLS_RESULT,'gateway replay never crosses into BYOK');
 }finally{await user('/byok/me',{method:'DELETE',expected:204});await owner('/config/byok',{method:'PATCH',body:{enabled:false}});}
}
export async function positiveYoutube(c){
 const {request:owner,body:baseline}=await configure(c);await owner('/config/tldr',{method:'PATCH',body:{...baseline,ai_route_mode:'openrouter'}});await owner('/access/45',{method:'POST',body:{lang:'en'},expected:201});const user=await session(c,45);
 try{for(const command of ['summary','about']){
  const body={command,url:VIDEO_URL},before=await count(c,'operator-completion'),sources=await count(c,'source');
  const first=await user('/summarize',{method:'POST',body});assert.equal(first.cached,false);assert.equal(first.result,TLS_RESULT);
  assert.equal(await count(c,'operator-completion'),before+1);assert((await count(c,'source'))>=sources+2,'actual watch + transcript extraction');
  assert((await c.tunnelEvents()).some(event=>event.ip===c.tls.dataAddress && event.port===443),'provider request traversed the owned SOCKS tunnel');
  await c.db.batch(`INSERT INTO insight_native_reservations(id,user_id,command,created_at,expires_at) VALUES ${Array.from({length:50},(_,i)=>`('tls-quota-${i}',45,'summary',unixepoch()*1000000,(unixepoch()+600)*1000000)`).join(',')};`);
  try{const extractionBefore=await count(c,'source');const replay=await user('/summarize',{method:'POST',body});assert.equal(replay.cached,true);assert.equal(replay.result,first.result);assert.equal(await count(c,'operator-completion'),before+1);assert.equal(await count(c,'source'),extractionBefore,'authorized replay returns before extraction');
   const denied=await user('/summarize',{method:'POST',body:{...body,focus:`fresh-quota-${command}`},expected:429});assert.equal(denied.detail,'quota_exceeded');
  }finally{await c.db.batch("DELETE FROM insight_native_reservations WHERE id LIKE 'tls-quota-%'");}
 }}finally{await owner('/access/45',{method:'DELETE',expected:204});await owner('/config/tldr',{method:'PATCH',body:baseline});}
}
export async function positiveConfinement(c){
 const {request}=await configure(c);const before=await count(c,'byok-completion');
 const denied=await request('/byok/me/test',{method:'POST',body:{provider:'custom_openai',base_url:'https://byok.fixture.invalid/v1',api_key:'fixture-byok-one'}});assert.equal(denied.ok,false);assert.equal(denied.error,'provider_probe_failed');
 for(const url of ['http://127.0.0.1/private','http://169.254.169.254/latest/meta-data/']){const error=await request('/summarize',{method:'POST',body:{command:'tldr',url,focus:'tls-source-denied'},expected:400});assert.equal(error.detail,'content_network_denied');}
 assert.equal(await count(c,'byok-completion'),before);
 await c.tls.verifyTlsRejections(); // Original image, verified TLS only.
}
