import test from 'node:test';
import assert from 'node:assert/strict';
import { options, owned, report, GATES } from './helpers.mjs';
import { Services, ARTICLE, CONTENT } from './services.mjs';
const prompt = `${CONTENT}\nCurrent UTC date: ${new Date().toISOString().slice(0,10)}\nYour training memory is not independent verification. If evidence is missing, say the claim is unverified, not false.`;
const image='sha256:'+'a'.repeat(64);
const args=['--server-image',image,'--fixture-image',image,'--worker-image',image];
test('only declared inputs, immutable images and unique known gates',{timeout:1000},()=>{
  assert.deepEqual(options(args).selected,GATES);
  assert.deepEqual(options([...args,'--scenarios','summary,shutdown']).selected,['summary','shutdown']);
  for(const extra of [['--scenarios','summary,summary'],['--scenarios','invented'],['--scenarios',''],['--privileged','true'],['--worker-image',image],['--chromium','relative']])assert.throws(()=>options([...args,...extra]));
  assert.throws(()=>options(['--server-image','latest','--fixture-image',image,'--worker-image',image]));assert.throws(()=>options([]));
  assert.equal(options([...args,'--frontend-image',image,'--chromium','/usr/bin/chromium']).chromium,'/usr/bin/chromium');
});
test('registration before setup, reverse cleanup and primary failure preserved',{timeout:1000},async()=>{
  const order=[],setup=new Error('allocation failed'),cleanup=new Error('cleanup failed');
  await assert.rejects(owned(async defer=>{defer(()=>{order.push('first');throw cleanup;});defer(()=>order.push('second'));throw setup;}),e=>{assert.deepEqual(e.errors,[setup,cleanup]);return true;});
  assert.deepEqual(order,['second','first']);
});
test('cancellation still drains every owned cleanup; success returns value',{timeout:1000},async()=>{
  const cancelled=new Error('cancelled'),calls=[];
  await assert.rejects(owned(async defer=>{defer(()=>calls.push('volume'));defer(()=>calls.push('container'));throw cancelled;}),e=>e.errors[0]===cancelled);
  assert.deepEqual(calls,['container','volume']);assert.equal(await owned(async defer=>{defer(()=>calls.push('done'));return 42;}),42);
});
test('diagnostics include aggregate/cause, redact credentials and bound size',{timeout:1000},()=>{
  const secret='fixture-private-key';const e=new AggregateError([new Error(secret),new Error('cleanup')],'primary',{cause:new Error('cause')});
  const text=report(e,[secret]);assert(!text.includes(secret));assert(text.includes('cleanup'));assert(text.includes('cause'));assert(report('x'.repeat(20000)).length<=12000);
});
test('operator proxy is exact public source only and denies unknown traffic',{timeout:3000},async()=>{
  await owned(async defer=>{
    const service=new Services();defer(()=>service.close());await service.start();
    // Absolute-form request as produced by a declared HTTP proxy client.
    const http=await import('node:http');
    const request=path=>new Promise((resolve,reject)=>{const req=http.request(service.origin,{path,method:'GET'},res=>{let body='';res.on('data',c=>body+=c);res.on('end',()=>resolve({status:res.statusCode,body}));});req.setTimeout(1000,()=>req.destroy(new Error('deadline')));req.once('error',reject);req.end();});
    const article=await request(ARTICLE);assert.equal(article.status,200);assert(article.body.includes(CONTENT));
    for(const path of ['/article','http://127.0.0.1/article',ARTICLE+'?extra=1','http://93.184.216.34/unknown'])assert.equal((await request(path)).status,403);
    assert.equal((await fetch(service.origin+'/v1/unknown',{method:'POST',signal:AbortSignal.timeout(1000)})).status,403);
  });
});
test('provider validates key/model/content; fragmented stream and held work cleanup',{timeout:3000},async()=>{
  const service=new Services();await service.start();
  try {
    const call=body=>fetch(service.origin+'/v1/chat/completions',{method:'POST',headers:{authorization:'Bearer fixture-gateway'},body:JSON.stringify(body),signal:AbortSignal.timeout(1000)});
    assert.equal((await call({model:'unknown',messages:[]})).status,400);
    assert.equal((await call({model:'fixture-model',messages:[]})).status,400);
    assert.equal((await call({model:'fixture-model',messages:[{content:CONTENT}]})).status,400);
    const response=await call({model:'fixture-model',messages:[{content:prompt}],stream:true});assert.equal(response.status,200);const wire=await response.text();assert(wire.includes('Native café summary.'));assert(wire.endsWith('data: [DONE]\n\n'));
    service.holdProvider=true;
    const pending=call({model:'fixture-model',messages:[{content:prompt}]}).then(r=>r.json());
    const providerDeadline=Date.now()+500;while(service.providers.size===0 && Date.now()<providerDeadline)await new Promise(r=>setTimeout(r,10));assert.equal(service.providers.size,1);
    service.releaseProviders();assert.equal((await pending).choices[0].message.content,'Native café summary.');service.holdProvider=false;
    service.holdContent=true;
    const http=await import('node:http');
    const req=http.request(service.origin,{path:ARTICLE},()=>{});req.on('error',()=>{});req.end();
    const end=Date.now()+500;while(service.held.size===0 && Date.now()<end)await new Promise(r=>setTimeout(r,10));assert.equal(service.held.size,1);
    await service.close();assert.equal(service.sockets.size,0);
  }finally{if(service.server.listening)await service.close();}
});
test('CONNECT serves only the owned article authority and never forwards',{timeout:3000},async()=>{
  await owned(async defer=>{
    const service=new Services();defer(()=>service.close());await service.start();
    const http=await import('node:http');
    const get=(target,path,host='93.184.216.34')=>new Promise((resolve,reject)=>{
      const request=http.request(service.origin,{method:'CONNECT',path:target});request.once('error',reject);
      request.setTimeout(1000,()=>request.destroy(new Error('CONNECT deadline')));
      request.once('connect',(response,socket)=>{
        if(response.statusCode!==200){socket.destroy();resolve(String(response.statusCode));return;}
        let body='';socket.setTimeout(1000,()=>socket.destroy(new Error('tunnel deadline')));socket.once('error',reject);
        socket.on('data',chunk=>body+=chunk);socket.once('end',()=>resolve(body));
        socket.end(`GET ${path} HTTP/1.1\r\nHost: ${host}\r\nConnection: close\r\n\r\n`);
      });request.end();
    });
    assert((await get('93.184.216.34:80','/article')).includes(CONTENT));
    assert.match(await get('93.184.216.34:80','/llms.txt'),/^HTTP\/1.1 404 /);
    assert.match(await get('93.184.216.34:80','/article','127.0.0.1'),/^HTTP\/1.1 403 /);
    assert.equal(await get('127.0.0.1:80','/article'),'403');
  });
});

test('every declared gate has an executable implementation',{timeout:1000},async()=>{
  const scenarios=await import('./scenarios.mjs'),advanced=await import('./advanced.mjs'),oauth=await import('./oauth.mjs'),spa=await import('./spa.mjs');
  const forced=await import('./forced-receipt.mjs');
  const positive=await import('./tls-scenarios.mjs');
  const all={...scenarios,...advanced,...oauth,...spa,...forced,...positive};for(const gate of GATES)assert.equal(typeof all[gate],'function',gate);
});
test('owned OAuth validates client/scope, denied write scope, and exact paths',{timeout:3000},async()=>{
  await owned(async defer=>{
    const service=new Services();defer(()=>service.close());await service.start();
    const post=async(path,body)=>{const response=await fetch(service.origin+path,{method:'POST',body:JSON.stringify(body),signal:AbortSignal.timeout(1000)});return {status:response.status,body:response.status===200?await response.json():null};};
    assert.equal((await post('/login/device/code',{client_id:'fixture-write-client',scope:'repo'})).status,400);
    service.oauthPlans.push({mode:'missingScope'});
    const start=await post('/login/device/code',{client_id:'fixture-write-client',scope:'public_repo'});assert.equal(start.status,200);
    const body={client_id:'fixture-write-client',device_code:start.body.device_code,grant_type:'urn:ietf:params:oauth:grant-type:device_code'};
    const poll=await post('/login/oauth/access_token',body);assert.equal(poll.status,200);assert.equal(poll.body.scope,'read:user');
    assert.equal((await post('/login/oauth/access_token',{...body,client_id:'fixture-read-client'})).status,400);
    assert.equal((await fetch(service.origin+'/user',{signal:AbortSignal.timeout(1000)})).status,400);
    assert.equal((await fetch(service.origin+'/user',{headers:{authorization:'token fixture-oauth-write'},signal:AbortSignal.timeout(1000)})).status,200);
    assert.equal((await fetch(service.origin+'/login/unknown',{signal:AbortSignal.timeout(1000)})).status,403);
  });
});
test('held OAuth polling supports release and socket teardown without live providers',{timeout:3000},async()=>{
  const service=new Services();await service.start();
  try {
    service.oauthPlans.push({mode:'hold',token:'fixture-oauth-late'});
    const request=(path,body)=>fetch(service.origin+path,{method:'POST',body:JSON.stringify(body),signal:AbortSignal.timeout(1000)}).then(r=>r.json());
    const start=await request('/login/device/code',{client_id:'fixture-read-client',scope:'read:user'});
    const pending=request('/login/oauth/access_token',{client_id:'fixture-read-client',device_code:start.device_code,grant_type:'urn:ietf:params:oauth:grant-type:device_code'});
    const deadline=Date.now()+500;while(service.polls.size===0&&Date.now()<deadline)await new Promise(r=>setTimeout(r,5));assert.equal(service.polls.size,1);
    service.releasePolls();assert.equal((await pending).access_token,'fixture-oauth-late');assert.equal(service.polls.size,0);
    await service.close();assert.equal(service.sockets.size,0);
  } finally {if(service.server.listening)await service.close();}
});
test('deterministic content/provider failures remain non-successes',{timeout:3000},async()=>{
  await owned(async defer=>{
    const service=new Services();defer(()=>service.close());await service.start();service.providerFailure=true;
    const response=await fetch(service.origin+'/v1/chat/completions',{method:'POST',headers:{authorization:'Bearer fixture-gateway'},body:JSON.stringify({model:'fixture-model',messages:[{content:prompt}]}),signal:AbortSignal.timeout(1000)});
    assert.equal(response.status,503);assert.equal(service.calls.at(-1).kind,'completion');
    service.contentFailure=true;
    const http=await import('node:http');const status=await new Promise((resolve,reject)=>{const req=http.request(service.origin,{path:ARTICLE},r=>{r.resume();resolve(r.statusCode);});req.on('error',reject);req.end();});assert.equal(status,503);
  });
});
test('malformed and held OAuth START responses are deterministic across replacement',{timeout:3000},async()=>{
  await owned(async defer=>{
    const service=new Services();defer(()=>service.close());await service.start();
    const post=()=>fetch(service.origin+'/login/device/code',{method:'POST',body:JSON.stringify({client_id:'fixture-read-client',scope:'read:user'}),signal:AbortSignal.timeout(1500)}).then(r=>r.json());
    service.oauthPlans.push({mode:'malformed'});assert.equal((await post()).user_code,42);
    service.oauthPlans.push({mode:'holdStart'});const old=post();
    const end=Date.now()+500;while(service.starts.size===0&&Date.now()<end)await new Promise(r=>setTimeout(r,5));assert.equal(service.starts.size,1);
    const current=await post();assert.equal(current.device_code,'device-3');assert.equal(current.user_code,'FIXTURE-3');
    service.releaseStarts();const late=await old;assert.equal(late.device_code,'device-2');assert.equal(late.user_code,'FIXTURE-2');assert.equal(service.starts.size,0);
  });
});
test('receipt stall retains real Bot API response and closes on abort',{timeout:3000},async()=>{
  const {BotApiFixture}=await import('../media-fixture/bot-api.mjs');
  await owned(async defer=>{
    const bot=new BotApiFixture({mediaBytes:Buffer.alloc(0)});defer(()=>bot.close());const port=await bot.start();
    const service=new Services();defer(()=>service.close());service.botOrigin=`http://127.0.0.1:${port}`;await service.start();service.holdReceipt=true;
    const send=signal=>fetch(service.origin+'/bot123456:fixture_insight_token/sendMessage',{method:'POST',body:JSON.stringify({chat_id:44,text:'receipt fixture'}),signal}).then(r=>r.json());
    const controller=new AbortController(),pending=send(controller.signal).then(value=>({value}),error=>({error}));
    const wait=async check=>{const end=Date.now()+500;while(!check()&&Date.now()<end)await new Promise(r=>setTimeout(r,5));assert(check());};
    await wait(()=>service.receipts.size===1);assert.equal(bot.calls.filter(x=>x.method==='sendmessage').length,1);
    controller.abort();assert((await pending).error);await wait(()=>service.receipts.size===0);
    const next=send(AbortSignal.timeout(1000));await wait(()=>service.receipts.size===1);service.releaseReceipts();
    const receipt=await next;assert.equal(receipt.ok,true);assert.equal(receipt.result.text,'receipt fixture');assert.equal(receipt.result.from.id,123456);assert.equal(service.receipts.size,0);
  });
});
