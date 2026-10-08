import assert from 'node:assert/strict';
import {session} from './scenarios.mjs';
const prefix=write=>write?'/github/upgrade-scope':'/github/login';
const tokenPath=write=>write?'/github/write-token':'/github/token';
export async function startFlow(c,user,plan,write=false) {
  c.trusted.oauthPlans.push(plan);
  const result=await user(prefix(write),{method:'POST'});
  assert.match(result.user_code,/^FIXTURE-\d+$/);assert.equal(result.status,'pending');assert.equal(result.interval,1);
  assert(result.expires_at>=Math.floor(Date.now()/1000)+(plan.expires??60)-2 && result.expires_at<=Math.floor(Date.now()/1000)+(plan.expires??60)+1,'bounded native expiry timestamp');
  assert.equal(result.verification_uri,'https://github.com/login/device','native URL remains fixed; fixture never visits provider');
  return result;
}
async function status(c,user,value,write=false) {
  return c.waitFor(async()=>{const view=await user(`${prefix(write)}/status`);return view.status===value?view:false;},`native OAuth ${write?'write':'read'} status ${value}`,20000);
}
async function absent(c,uid,column) {
  assert(['github_token','github_token_public_repo'].includes(column));
  assert.equal(await c.db.query(`SELECT coalesce(${column},'')='' FROM insight_user_settings WHERE user_id=${uid}`),'1','cancel/disconnect must leave no persisted OAuth token');
}
export async function oauth(c) {
  const user=await session(c,43);
  await user('/settings/me',{method:'PATCH',body:{lang:'en',github_token:''}});
  await user('/github/write-token',{method:'DELETE',expected:204});
  assert.equal((await user('/github/write-token/status')).configured,true);
  await startFlow(c,user,{mode:'success'});await status(c,user,'success');
  assert.equal((await user('/settings/me')).github_token_set,true);
  assert.equal(await c.db.query("SELECT github_token='fixture-oauth-read' AND coalesce(github_token_public_repo,'')='' FROM insight_user_settings WHERE user_id=43"),'1');
  await startFlow(c,user,{mode:'missingScope'},true);
  const invalid=await status(c,user,'error',true);assert.equal(invalid.error,'oauth_failed');
  await absent(c,43,'github_token_public_repo');assert.equal((await user('/settings/me')).github_token_set,true,'read token survives denied write scope');
  await startFlow(c,user,{mode:'success'},true);await status(c,user,'success',true);
  assert.equal(await c.db.query("SELECT github_token='fixture-oauth-read' AND github_token_public_repo='fixture-oauth-write' FROM insight_user_settings WHERE user_id=43"),'1','read and write credentials are independent');
  await user('/github/write-token',{method:'DELETE',expected:204});await absent(c,43,'github_token_public_repo');
  assert.equal((await user('/settings/me')).github_token_set,true);
  await user('/github/token',{method:'DELETE',expected:204});await absent(c,43,'github_token');
  await startFlow(c,user,{mode:'pending',expires:2});const expired=await status(c,user,'expired');assert.equal(expired.error,null);await absent(c,43,'github_token');
  await startFlow(c,user,{mode:'denied'});const denied=await status(c,user,'error');assert.equal(denied.error,'oauth_failed');await absent(c,43,'github_token');
  for(const write of [false,true])for(const disconnect of [false,true]) {
    const column=write?'github_token_public_repo':'github_token';
    await startFlow(c,user,{mode:'hold',token:'fixture-oauth-late'},write);
    try {
      await c.waitFor(()=>c.trusted.polls.size===1,'actual OAuth poll blocked at owned operator');
      if(disconnect) {
        await user(tokenPath(write),{method:'DELETE',expected:204});
        assert.equal((await user(`${prefix(write)}/status`)).status,'none');await absent(c,43,column);
      } else {
        // New start cancels the old generation; late old token cannot replace it.
        await startFlow(c,user,{mode:'success'},write);await status(c,user,'success',write);
      }
      c.trusted.releasePolls();await c.waitFor(()=>c.trusted.polls.size===0,'late OAuth poll response released');
      if(disconnect) {assert.equal((await user(`${prefix(write)}/status`)).status,'none');await absent(c,43,column);}
      else {assert.equal((await user(`${prefix(write)}/status`)).status,'success');assert.equal(await c.db.query(`SELECT ${column}='fixture-oauth-${write?'write':'read'}' FROM insight_user_settings WHERE user_id=43`),'1','late cancelled generation cannot overwrite current token');await user(tokenPath(write),{method:'DELETE',expected:204});await absent(c,43,column);}
    } finally {c.trusted.releasePolls();}
  }
  // Final positive barrier: the real poll loop still works after all cancellations.
  await startFlow(c,user,{mode:'success'});await status(c,user,'success');
  assert.equal(await c.db.query("SELECT github_token='fixture-oauth-read' FROM insight_user_settings WHERE user_id=43"),'1');
  await user('/github/token',{method:'DELETE',expected:204});await absent(c,43,'github_token');
}
export async function holdShutdownOAuth(c) {
  const user=await session(c,43);
  await user('/settings/me',{method:'PATCH',body:{lang:'en',github_token:''}});
  await user('/github/write-token',{method:'DELETE',expected:204});
  await startFlow(c,user,{mode:'hold',token:'fixture-oauth-late'});
  await c.waitFor(()=>c.trusted.polls.size===1,'live OAuth poll at original-process SIGTERM');
}
export async function checkShutdownOAuth(c) {
  c.trusted.releasePolls();await absent(c,43,'github_token');await absent(c,43,'github_token_public_repo');
}
// Public requests, not an imported flow map: exceed the unchanged 1024-slot
// bound with failed starts. Every failed admission must disappear immediately.
export async function oauthStart(c) {
  for(let start=5000;start<=6024;start+=100) {
    const values=Array.from({length:Math.min(100,6025-start)},(_,i)=>`(${start+i},'user','en','macchiato',0,0,0)`);
    await c.db.batch(`INSERT INTO users(id,role,language,theme,is_premium,created_at,updated_at) VALUES ${values.join(',')};`);
  }
  await c.db.batch("INSERT INTO user_permissions(user_id,plugin,feature,granted_by,granted_at) SELECT id,'insight','tldr',42,0 FROM users WHERE id BETWEEN 5000 AND 6024;");
  try {
    for(let uid=5000;uid<=6024;uid++) {
      const user=await session(c,uid);c.trusted.oauthPlans.push({mode:'malformed'});
      const failed=await user('/github/login',{method:'POST',expected:400});assert.equal(failed.detail,'oauth_invalid_response');
      assert.equal((await user('/github/login/status')).status,'none','malformed START cannot retain a capacity slot');
    }
    const user=await session(c,43),token=await c.login(43);
    for(const write of [false,true]) {
      await user(tokenPath(write),{method:'DELETE',expected:204});
      const rawStart=signal=>fetch(`http://127.0.0.1:${c.httpPort}/api/v1/insight${prefix(write)}`,{method:'POST',headers:{authorization:`Bearer ${token}`},signal});
      // Final authorization failure after admission and upstream I/O.
      c.trusted.oauthPlans.push({mode:'holdStart'});const finalAuth=rawStart(AbortSignal.timeout(7000)).catch(error=>({error}));
      try {
        await c.waitFor(()=>c.trusted.starts.size===1,'START held before final authorization');
        await c.db.batch("UPDATE users SET role='banned' WHERE id=43");c.trusted.releaseStarts();
        assert.equal((await finalAuth).status,403);
      } finally {await c.db.batch("UPDATE users SET role='user' WHERE id=43");c.trusted.releaseStarts();await finalAuth.catch(()=>{});}
      assert.equal((await user(`${prefix(write)}/status`)).status,'none','final authorization failure releases admission');
      // Replace while the old START future still owns its admission guard.
      // Its eventual cleanup must be generation-fenced, not remove the new flow.
      c.trusted.oauthPlans.push({mode:'holdStart',token:'fixture-oauth-late'});
      const replaced=rawStart(AbortSignal.timeout(7000)).then(async response=>({status:response.status,body:await response.json()}),error=>({error}));
      try {
        await c.waitFor(()=>c.trusted.starts.size===1,'old START guard live during replacement');
        await startFlow(c,user,{mode:'success'},write);const old=await replaced;
        assert.equal(old.status,400);assert.equal(old.body.detail,'cancelled');
        await status(c,user,'success',write);c.trusted.releaseStarts();
        await c.waitFor(()=>c.trusted.starts.size===0,'replaced START upstream released');
        assert.equal((await user(`${prefix(write)}/status`)).status,'success','old guard cannot remove replacement');
      } finally {c.trusted.releaseStarts();await replaced;}
      await user(tokenPath(write),{method:'DELETE',expected:204});
      // Actual client disconnect while START is awaiting the canned reply.
      c.trusted.oauthPlans.push({mode:'holdStart',token:'fixture-oauth-late'});const controller=new AbortController();
      const dropped=rawStart(controller.signal).then(r=>({status:r.status}),error=>({error}));
      try {
        await c.waitFor(()=>c.trusted.starts.size===1,'START admitted before client drop');
        const oldCode=c.trusted.calls.filter(x=>x.kind==='oauth-start').at(-1).code;controller.abort();assert((await dropped).error);
        await c.waitFor(async()=> (await user(`${prefix(write)}/status`)).status==='none','dropped START removes its generation',5000);
        // Keep the old upstream reply held until the replacement is successful.
        await startFlow(c,user,{mode:'success'},write);await status(c,user,'success',write);
        c.trusted.releaseStarts();await c.waitFor(()=>c.trusted.starts.size===0,'old START released after replacement');
        assert.equal((await user(`${prefix(write)}/status`)).status,'success');
        await c.stop(); // original process drains old request/guard/poller work
        assert.equal(await c.db.query(`SELECT ${write?'github_token_public_repo':'github_token'}='fixture-oauth-${write?'write':'read'}' FROM insight_user_settings WHERE user_id=43`),'1');
        assert(!c.trusted.calls.some(x=>x.kind==='oauth-poll' && x.code===oldCode),'dropped START never installs a late poller');
        await c.launch();await c.ready();
      } finally {controller.abort();c.trusted.releaseStarts();await dropped;}
      await user(tokenPath(write),{method:'DELETE',expected:204});
      await absent(c,43,write?'github_token_public_repo':'github_token');
    }
  } finally {c.trusted.releaseStarts();await c.db.batch('DELETE FROM users WHERE id BETWEEN 5000 AND 6024');}
}
