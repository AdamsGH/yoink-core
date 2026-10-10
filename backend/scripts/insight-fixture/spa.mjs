import assert from 'node:assert/strict';
import http from 'node:http';
import { isIPv4 } from 'node:net';
import { launchBrowser } from '../media-fixture/browser-client.mjs';
import { owned } from './helpers.mjs';
import { configure } from './scenarios.mjs';
import { seedByok } from './advanced.mjs';
export async function spa(c) {
  assert(c.config['frontend-image'],'DEPENDENCY: immutable coordinator-built frontend image required');
  return owned(async defer=>{
    const {request,body}=await configure(c);
    c.addSecret('fixture-unused-key');
    defer(()=>request('/config/tldr',{method:'PATCH',body}));
    await request('/config/tldr',{method:'PATCH',body:{...body,ai_route_mode:'auto',openrouter_api_key:'fixture-unused-key'}});
    const frontend=await c.startContainer(['--network',c.network,'--security-opt','no-new-privileges','--pids-limit','64','--memory','128m',c.config['frontend-image']]);
    const frontendAddress=await c.docker(['inspect','--format',`{{(index .NetworkSettings.Networks "${c.network}").IPAddress}}`,frontend]);assert(isIPv4(frontendAddress),'owned frontend IPv4');
    await c.waitFor(async()=>{try{return (await fetch(`http://${frontendAddress}/`,{signal:AbortSignal.timeout(1000)})).ok;}catch{return false;}},'built frontend readiness');
    const token=await c.login(42),calls=[],sockets=new Set();
    const server=http.createServer((req,res)=>{
      if(req.url==='/__fixture_session'){res.setHeader('content-type','text/html');res.end('<!doctype html><title>Fixture session</title>');return;}
      const backend=req.url.startsWith('/api/') || req.url.startsWith('/health/');
      let credentialEdits;
      if(['PATCH','PUT','POST'].includes(req.method) && req.url.startsWith('/api/v1/insight/')) {
        let raw='';req.on('data',chunk=>{raw+=chunk;if(Buffer.byteLength(raw)>100000)req.destroy();});
        req.on('end',()=>{try {const body=JSON.parse(raw);const keys=req.url.endsWith('/config/tldr')?['gateway_api_key','openrouter_api_key']:req.url.includes('/byok/me')?['api_key']:req.url.endsWith('/settings/me')?['github_token']:[];credentialEdits=Object.fromEntries(keys.map(key=>[key,!Object.hasOwn(body,key)?'omitted':body[key]===''?'clear':'replace']));}catch{credentialEdits={invalid:true};}raw='';});
      }
      const upstream=http.request(`http://${backend?`127.0.0.1:${c.httpPort}`:`${frontendAddress}:80`}${req.url}`,{method:req.method,headers:req.headers},reply=>{
        if(backend)calls.push({path:req.url,method:req.method,status:reply.statusCode,authenticated:req.headers.authorization===`Bearer ${token}`,credentialEdits});
        res.writeHead(reply.statusCode,reply.headers);reply.pipe(res);
      });upstream.once('error',()=>{res.statusCode=502;res.end();});req.pipe(upstream);res.once('close',()=>upstream.destroy());
    });
    server.on('connection',s=>{sockets.add(s);s.once('close',()=>sockets.delete(s));});
    defer(async()=>{for(const s of sockets)s.destroy();await new Promise((resolve,reject)=>server.close(e=>e?reject(e):resolve()));});
    await new Promise((resolve,reject)=>{server.once('error',reject);server.listen(0,'127.0.0.1',resolve);});
    const origin=`http://127.0.0.1:${server.address().port}`;
    const browser=await launchBrowser({origin,executablePath:c.config.chromium??'/usr/bin/chromium'});defer(()=>browser.close());
    await browser.request('Page.addScriptToEvaluateOnNewDocument',{source:`window.__fixtureErrors=[];addEventListener('error',e=>{if(window.__fixtureErrors.length<8)window.__fixtureErrors.push(String(e.message).slice(0,512));});`});
    await browser.navigate(origin+'/__fixture_session');await browser.evaluate(`localStorage.setItem('access_token',${JSON.stringify(token)});localStorage.setItem('yoink_lang','en');`);
    for(const [path,contracts] of [['/insight/settings',['settings/me','byok/me']],['/admin/insight-tldr',['config/tldr','config/byok']]]){
      const before=calls.length;await browser.navigate(origin+path);
      try{await c.waitFor(()=>contracts.every(suffix=>calls.slice(before).some(x=>x.path===`/api/v1/insight/${suffix}` && x.status===200 && x.authenticated)),`built SPA ${path} authenticated requests`);}catch(error){throw new Error(`${path}: ${JSON.stringify(calls.slice(before))}\n${await browser.evaluate('document.body.innerText.slice(0,1500)')}\n${JSON.stringify(await browser.evaluate('window.__fixtureErrors')).replaceAll(token,'[fixture-token]')}`,{cause:error});}
      assert.equal(await browser.evaluate('location.pathname'),path);
    }
    // Current integrated CredentialInput IDs, real React input events and
    // clear/undo buttons. Observe semantic presence only, never credential values.
    const save=async expected=>{
      const before=calls.length;
      await c.waitFor(()=>browser.evaluate(`!!document.querySelector('#tldr-gateway-url')`),'admin form rendered');
      await browser.evaluate(`(()=>{if([...document.querySelectorAll('button')].some(b=>b.textContent.trim()==='Save'))return;const e=document.querySelector('#tldr-gateway-url');Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(e,e.value.endsWith('/')?e.value.slice(0,-1):e.value+'/');e.dispatchEvent(new Event('input',{bubbles:true}));})()`);
      try{await c.waitFor(()=>browser.evaluate(`!![...document.querySelectorAll('button')].find(b=>b.textContent.trim()==='Save'&&!b.disabled)`),'admin form ready to save');}catch(error){throw new Error(await browser.evaluate('document.body.innerText.slice(0,3500)'),{cause:error});}
      await browser.evaluate(`(()=>{const b=[...document.querySelectorAll('button')].find(b=>b.textContent.trim()==='Save');if(!b||b.disabled)throw Error('missing enabled admin Save');b.click();})()`);
      const call=await c.waitFor(()=>calls.slice(before).find(x=>x.path==='/api/v1/insight/config/tldr' && x.method==='PATCH'),'actual SPA config PATCH');
      assert.equal(call.status,200);assert.deepEqual(call.credentialEdits,expected);
      await c.waitFor(()=>browser.evaluate(`![...document.querySelectorAll('button')].some(b=>b.textContent.trim()==='Save')`),'SPA save reset');
    };
    const omitted={gateway_api_key:'omitted',openrouter_api_key:'omitted'};
    await save(omitted);
    await browser.evaluate(`(()=>{const e=document.querySelector('#tldr-gateway-key');if(!e)throw Error('missing gateway input');Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(e,'fixture-gateway');e.dispatchEvent(new Event('input',{bubbles:true}));})()`);
    await save({gateway_api_key:'replace',openrouter_api_key:'omitted'});await save(omitted);
    const clickCredential=label=>browser.evaluate(`(()=>{const e=document.querySelector('#tldr-gateway-key');const b=[...e.parentElement.querySelectorAll('button')].find(b=>b.textContent.trim()===${JSON.stringify(label)});if(!b)throw Error('missing credential action');b.click();})()`);
    await clickCredential('Clear stored key');
    assert.equal(await browser.evaluate(`document.querySelector('#tldr-gateway-key').disabled`),true);
    await clickCredential('Undo key change');await save(omitted);
    await clickCredential('Clear stored key');await save({gateway_api_key:'clear',openrouter_api_key:'omitted'});
    assert.equal((await request('/config/tldr')).gateway_api_key_set,false,'actual form explicit empty clears native stored key');await save(omitted);
    await browser.navigate(origin+'/insight/settings');
    assert.equal(await browser.evaluate("localStorage.getItem('access_token')"),token,'SPA reload retains real login');
    await settingsForms(c,{browser,origin,calls,request});
  });
}

async function settingsForms(c,{browser,origin,calls,request}) {
  const navigate=async()=>{const before=calls.length;await browser.navigate(origin+'/insight/settings');await c.waitFor(()=>calls.slice(before).some(x=>x.path.endsWith('/settings/me')&&x.status===200),'SPA settings reload');await c.waitFor(()=>browser.evaluate(`!!document.querySelector('[data-slot="card-title"]')`),'SPA settings rendered');};
  const input=async(selector,value)=>browser.evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(selector)});if(!e)throw Error('missing form input');Object.getOwnPropertyDescriptor(e instanceof HTMLTextAreaElement?HTMLTextAreaElement.prototype:HTMLInputElement.prototype,'value').set.call(e,${JSON.stringify(value)});e.dispatchEvent(new Event('input',{bubbles:true}));})()`);
  const click=async(label,scope='document')=>browser.evaluate(`(()=>{const root=${scope};if(!root)throw Error('missing form scope');const b=[...root.querySelectorAll('button')].find(b=>b.textContent.trim()===${JSON.stringify(label)}&&!b.disabled);if(!b)throw Error('missing enabled form action');b.click();})()`);
  const observed=async(path,method,action,edits)=>{const before=calls.length;await action();const call=await c.waitFor(()=>calls.slice(before).find(x=>x.path===`/api/v1/insight${path}`&&x.method===method),`SPA ${method} ${path}`);assert.equal(call.status,method==='DELETE'?204:method==='POST'&&path==='/aliases'?201:200);if(edits)assert.deepEqual(call.credentialEdits,edits);};
  const githubCard=`[...document.querySelectorAll('[data-slot="card-title"]')].find(e=>e.textContent.includes('GitHub token'))?.closest('.bg-card')`;
  const githubInput=`.bg-card:has([data-slot="card-title"]) input[type="password"]:not(#byok-api-key)`;
  await request('/config/tldr',{method:'PATCH',body:{allowed_models:['fixture-model'],default_model:'fixture-model',ai_route_mode:'a2a',gateway_base_url:c.trusted.origin+'/v1',gateway_api_key:'fixture-gateway'}});
  await navigate();
  // GitHub replace -> reset -> unrelated prompt save omission -> clear/undo.
  await input(githubInput,'fixture-oauth-read');await observed('/settings/me','PATCH',()=>click('Save'),{github_token:'replace'});
  await navigate();
  await browser.evaluate(`(()=>{if(document.querySelector('#prompt-tldr'))return;const b=[...document.querySelectorAll('button[aria-expanded]')].find(e=>e.textContent.includes('/tldr'));if(!b)throw Error('missing TLDR prompt trigger');b.click();})()`);
  await c.waitFor(()=>browser.evaluate(`!!document.querySelector('#prompt-tldr')`),'expanded prompt editor');
  await input('#prompt-tldr','SPA unrelated prompt save.');await observed('/settings/me','PATCH',()=>click('Save'),{github_token:'omitted'});
  assert.equal((await request('/settings/me')).github_token_set,true);
  await click('Clear stored key',githubCard);await click('Undo key change',githubCard);
  await click('Clear stored key',githubCard);await observed('/settings/me','PATCH',()=>click('Save'),{github_token:'clear'});
  assert.equal((await request('/settings/me')).github_token_set,false);
  // Real alias dialog creation/update. Fixed UI data contains no credentials.
  await navigate();
  const aliasCard=`[...document.querySelectorAll('[data-slot="card-title"]')].find(e=>e.textContent.includes('Use /tldr'))?.closest('.bg-card')`;
  await browser.evaluate(`(()=>{const b=(${aliasCard})?.querySelector('button');if(!b)throw Error('missing alias add');b.click();})()`);
  await c.waitFor(()=>browser.evaluate(`!!document.querySelector('#alias-prompt')`),'real alias dialog');
  await input('input[placeholder="deep, brief, tech ..."]','spa_fixture,');await input('#alias-prompt','SPA fixture concise prose.');
  await observed('/aliases','POST',()=>click('Save',`document.querySelector('[role="dialog"]')`));
  const alias=(await request('/aliases')).find(x=>x.aliases==='spa_fixture');assert(alias);
  try {
    await c.waitFor(()=>browser.evaluate(`!document.querySelector('#alias-prompt')`),'alias dialog closes');
    await browser.evaluate(`(()=>{const tag=[...document.querySelectorAll('div.rounded-full')].find(e=>e.textContent.trim()==='spa_fixture');const row=tag?.closest('.flex.items-start.gap-3');const b=row?.querySelector('button');if(!b)throw Error('missing alias edit');b.click();})()`);
    await c.waitFor(()=>browser.evaluate(`!!document.querySelector('#alias-prompt')`),'alias edit opens');await input('#alias-prompt','SPA fixture edited prose.');
    await observed(`/aliases/${alias.id}`,'PATCH',()=>click('Save',`document.querySelector('[role="dialog"]')`));
    assert.equal((await request('/aliases')).find(x=>x.id===alias.id).prompt,'SPA fixture edited prose.');
  } finally {await request(`/aliases/${alias.id}`,{method:'DELETE',expected:204});}
  // Imported readiness is solely form setup. Every save/probe invokes native
  // policy: this private-address endpoint must fail its real probe safely.
  await request('/config/byok',{method:'PATCH',body:{enabled:true}});await seedByok(c,42,{ready:true});
  try {
    await navigate();await c.waitFor(()=>browser.evaluate(`!!document.querySelector('#byok-api-key')`),'BYOK editor');
    const card=`document.querySelector('#byok-api-key')?.closest('.bg-card')`;
    await observed('/byok/me','PUT',async()=>{await click('Save',card);await c.waitFor(()=>browser.evaluate(`!!document.querySelector('[role="alertdialog"]')`),'BYOK non-search model warning');await click('Continue',`document.querySelector('[role="alertdialog"]')`);},{api_key:'omitted'});
    assert.equal(await c.db.query("SELECT api_key='fixture-byok-one' FROM insight_user_byok WHERE user_id=42"),'1');
    await observed('/byok/me/test','POST',()=>click('Test connection',card),{api_key:'omitted'});
    await input('#byok-api-key','fixture-byok-two');
    await observed('/byok/me/test','POST',()=>click('Test connection',card),{api_key:'replace'});
    await observed('/byok/me','PUT',async()=>{await click('Save',card);await c.waitFor(()=>browser.evaluate(`!!document.querySelector('[role="alertdialog"]')`),'BYOK replacement warning');await click('Continue',`document.querySelector('[role="alertdialog"]')`);},{api_key:'replace'});
    assert.equal(await c.db.query("SELECT api_key='fixture-byok-two' FROM insight_user_byok WHERE user_id=42"),'1');
    await input('#byok-api-key','fixture-byok-one');
    await click('Undo key change',`document.querySelector('#byok-api-key').parentElement`);
    await click('Clear stored key',`document.querySelector('#byok-api-key').parentElement`);
    assert.equal(await browser.evaluate(`document.querySelector('#byok-api-key').disabled`),true);
    await observed('/byok/me','PUT',()=>click('Save',card),{api_key:'clear'});assert.equal((await request('/byok/me')).api_key_set,false);
  } finally {await c.db.batch('DELETE FROM insight_user_byok WHERE user_id=42');await request('/config/byok',{method:'PATCH',body:{enabled:false}});}
  // OAuth form really starts and polls; never open the external verification URL.
  await navigate();c.trusted.oauthPlans.push({mode:'success'});
  const before=calls.length;await click('Sign in with GitHub',githubCard);
  await c.waitFor(()=>calls.slice(before).some(x=>x.path.endsWith('/github/login')&&x.method==='POST'&&x.status===200),'SPA device start');
  await c.waitFor(()=>calls.slice(before).some(x=>x.path.endsWith('/github/login/status')&&x.status===200),'SPA live device polling');
  try {
    await c.waitFor(()=>browser.evaluate(`!document.querySelector('[role="dialog"]')`),'SPA recognizes native successful OAuth status and closes form',15000);
    assert.equal((await request('/settings/me')).github_token_set,true);
    c.trusted.oauthPlans.push({mode:'success'});
    const writeCard=`[...document.querySelectorAll('[data-slot="card-title"]')].find(e=>e.textContent.includes('GitHub write access'))?.closest('.bg-card')`;
    await observed('/github/upgrade-scope','POST',()=>click('Connect',writeCard));
    await c.waitFor(()=>browser.evaluate(`!document.querySelector('[role="dialog"]') && [...document.querySelectorAll('button')].some(b=>b.textContent.trim()==='Revoke')`),'SPA write OAuth closes form and shows revoke');
    assert.equal((await request('/github/write-token/status')).enabled,true);
    await observed('/github/write-token','DELETE',()=>click('Revoke',writeCard));
    assert.equal((await request('/github/write-token/status')).enabled,false);assert.equal((await request('/settings/me')).github_token_set,true,'SPA write revoke preserves read credential');
  } finally {await request('/github/token',{method:'DELETE',expected:204});await request('/github/write-token',{method:'DELETE',expected:204});}
  assert.equal((await request('/settings/me')).github_token_set,false);
}
