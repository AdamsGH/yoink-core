import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import net from 'node:net';
import { startHeldDns, resolverArgs } from '../inbox-fixture/dns-harness.mjs';
import { readFile } from 'node:fs/promises';
import { createProcessRunner } from '../media-fixture/process-lifecycle.mjs';
import { createNativeDbClient } from '../media-fixture/native-db.mjs';
import { BotApiFixture } from '../media-fixture/bot-api.mjs';
import { owned, api, freePort, initData, waitFor, report } from './helpers.mjs';
import { Services } from './services.mjs';
import { createTlsTopology, occupiedSubnets } from './tls-topology.mjs';
export async function withRuntime(config, execute) {
  const secrets = [];
  try { return await owned(async defer => {
    const id=randomBytes(8).toString('hex'), volume=`yoink-insight-${id}`,network=`yoink-insight-net-${id}`;
    const children=new Set(), containers=new Set();
    const {capture,docker}=createProcessRunner({children});
    let helperNumber=0;
    async function helperContainer(args,timeout=30000,input) {
      if(input!==undefined)assert(Buffer.byteLength(input)<=1024*1024,'helper input bound');
      const name=`yoink-insight-helper-${id}-${helperNumber++}`;containers.add(name);
      return owned(async cleanup=>{
        cleanup(async()=>{const found=await docker(['ps','-aq','--filter',`name=^/${name}$`]);if(found)await docker(['rm','--force',name]);containers.delete(name);});
        const cid=await docker(['create','--pull','never','--name',name,...(input===undefined?[]:['--interactive']),...args]);assert.match(cid,/^[0-9a-f]{12,64}$/i);
        const result=await capture('docker',['start','--attach',...(input===undefined?[]:['--interactive']),cid],{timeout,input});
        assert.equal(await docker(['inspect','--format','{{.State.ExitCode}}',cid]),'0','helper failed');return result.output.trim();
      });
    }
    // Create by predetermined name and register BEFORE docker allocates, including
    // setup cancellation/nonzero output. Removal of absent names is harmless.
    let containerNumber=0;
    async function startContainer(args) {
      const index=args.indexOf('--name');
      const name=index<0?`yoink-insight-${id}-${containerNumber++}`:args[index+1];
      assert(name.startsWith(`yoink-insight-`),'owned container name');containers.add(name);
      const container=await docker(['run','--detach','--pull','never',...(index<0?['--name',name]:[]),...args]);
      assert.match(container,/^[0-9a-f]{12,64}$/i,'owned container ID');return container;
    }
    defer(async()=>{await Promise.all([...children].map(child=>new Promise((resolve,reject)=>{
      const timer=setTimeout(()=>reject(new Error('owned Node child did not reap')),2000);
      child.once('close',()=>{clearTimeout(timer);resolve();});child.kill('SIGKILL');
    })));});
    defer(async()=>{const names=await docker(['volume','ls','--format','{{.Name}}']);if(names.split('\n').includes(volume))await docker(['volume','rm',volume]);});
    defer(async()=>{const names=await docker(['network','ls','--format','{{.Name}}']);if(names.split('\n').includes(network))await docker(['network','rm',network]);});
    defer(async()=>{const failures=[];for(const name of containers) try {const found=await docker(['ps','-aq','--filter',`name=^/${name}$`]);if(found)await docker(['rm','--force',name]);}catch(e){failures.push(e);}if(failures.length) throw new AggregateError(failures,'container cleanup');});
    let app, relay, tls, appUsesTls=false, stoppedLogs='', interrupted=false;
    const interrupt=()=>{interrupted=true;if(app) void docker(['kill','--signal','TERM',app]).catch(()=>{});};
    for(const signal of ['SIGINT','SIGTERM']) process.once(signal,interrupt);
    defer(()=>{for(const signal of ['SIGINT','SIGTERM']) process.removeListener(signal,interrupt);});
    const check=()=>assert(!interrupted,'fixture cancelled');
    await docker(['network','create','--internal',network]);
    const topology=JSON.parse(await docker(['network','inspect',network]));assert.equal(topology[0].Internal,true,'no public egress');
    const gateway=topology[0].IPAM.Config[0].Gateway;assert.match(gateway,/^\d+\.\d+\.\d+\.\d+$/);
    const bot=new BotApiFixture({mediaBytes:Buffer.alloc(0)});defer(()=>bot.close());const botPort=await bot.start();
    const trusted=new Services();trusted.botOrigin=`http://127.0.0.1:${botPort}`;defer(()=>trusted.close());await trusted.start(gateway);
    const fixturePort=await freePort(), httpPort=await freePort(), oauthPort=await freePort();
    let appAddress;
    const apiSockets=new Set();
    const apiRelay=net.createServer(socket=>{
      apiSockets.add(socket);socket.once('close',()=>apiSockets.delete(socket));
      if(!appAddress){socket.destroy();return;}
      const upstream=net.connect(httpPort,appAddress);apiSockets.add(upstream);upstream.once('close',()=>apiSockets.delete(upstream));
      for(const stream of [socket,upstream]){stream.setTimeout(30000,()=>stream.destroy());stream.on('error',()=>{socket.destroy();upstream.destroy();});}
      socket.pipe(upstream).pipe(socket);socket.once('close',()=>upstream.destroy());upstream.once('close',()=>socket.destroy());
    });
    defer(async()=>{for(const socket of apiSockets)socket.destroy();if(apiRelay.listening)await new Promise(resolve=>apiRelay.close(resolve));});
    await new Promise((resolve,reject)=>{apiRelay.once('error',reject);apiRelay.listen(httpPort,'127.0.0.1',resolve);});
    const secret=randomBytes(32).toString('hex'), fixtureToken=randomBytes(32).toString('hex'),telegramToken='123456:fixture_insight_token';secrets.push(secret,fixtureToken,telegramToken,'fixture-gateway','fixture-management-key','fixture-gateway-two','fixture-oauth-read','fixture-oauth-write','fixture-oauth-late','fixture-byok-one','fixture-byok-two');
    const mount=['--mount',`type=volume,source=${volume},target=/state`];
    for(const image of new Set([config['server-image'],config['fixture-image'],config['worker-image']])) await docker(['image','inspect',image]);
    const fingerprints=[];
    for(const image of [config['server-image'],config['fixture-image'],config['worker-image']]) fingerprints.push(await helperContainer(['--network','none','--entrypoint','sh',image,'-c','sha256sum /opt/yoink-worker/*.py /usr/local/bin/yoink-worker-launcher']));
    assert(fingerprints.every(x=>x===fingerprints[0]),'immutable embedded worker/launcher mismatch');
    await docker(['volume','create',volume]);
    await helperContainer(['--network','none','--user','0',...mount,'--entrypoint','sh',config['server-image'],'-c','mkdir -p /state/private /state/insight; chmod 0700 /state/private /state/insight; chown 10001:10001 /state/private /state/insight']);
    await helperContainer(['--network','none',...mount,'--env','YOINK_RUST_DATABASE_PATH=/state/private/state.db','--entrypoint','yoink-server',config['server-image'],'migrate']);
    const socketClient=await readFile(new URL('./socket-client.py',import.meta.url),'utf8');
    const db=createNativeDbClient({getAppContainerId:()=>undefined,getFixturePort:()=>fixturePort,fixtureToken,volumeName:volume,fixtureImage:config['fixture-image'],helperContainer:async(args,timeout,input)=>app
      ? (await capture('docker',['exec','--interactive',app,'/usr/local/bin/python','-I','-B','-c',socketClient,String(fixturePort)],{timeout,input})).output.trim()
      : helperContainer(args,timeout,input)});
    await db.batch("INSERT INTO users(id,role,language,theme,is_premium,created_at,updated_at) VALUES(42,'owner','en','macchiato',0,0,0),(43,'user','en','macchiato',0,0,0),(44,'user','en','macchiato',0,0,0),(45,'user','en','macchiato',0,0,0); INSERT INTO user_permissions(user_id,plugin,feature,granted_by,granted_at) VALUES(43,'insight','tldr',42,0),(44,'insight','tldr',42,0);");
    const relaySource=await readFile(new URL('./operator-relay.py',import.meta.url),'utf8');
    if(config.selected?.some(name=>name.startsWith('positive'))) {
      const routes=async()=>{
        const host=JSON.parse((await capture('ip',['-j','-4','route','show','table','all'])).output);
        const networks=(await docker(['network','ls','-q'])).split('\n').filter(Boolean);
        const inspected=networks.length?JSON.parse(await docker(['network','inspect',...networks])):[];
        return occupiedSubnets(host,inspected.flatMap(network=>network.IPAM.Config??[]).map(entry=>entry.Subnet).filter(subnet=>subnet && !subnet.includes(':')));
      };
      tls=await createTlsTopology({docker,helperContainer,startContainer,defer,routes,image:config['fixture-image'],waitFor});
      await docker(['network','connect',network,tls.networkContainer]);
      // Stopped originals retain network attachments until container removal.
      defer(async()=>{for(const name of containers){const found=await docker(['ps','-aq','--filter',`name=^/${name}$`]);if(found)await docker(['rm','--force',name]);}});
    }
    async function launch({insight=true,inbox=false,dns,tls:useTls=false}={}) {
      check();assert(!app);
      assert(!useTls || tls,'owned TLS topology required');appUsesTls=useTls;
      stoppedLogs='';
      const env={YOINK_FIXTURE_BIND:`127.0.0.1:${fixturePort}`,YOINK_FIXTURE_TOKEN:fixtureToken,YOINK_RUST_DATABASE_PATH:'/state/private/state.db',YOINK_RUST_HTTP_BIND:`0.0.0.0:${httpPort}`,YOINK_RUST_AUTH_ENABLED:'true',YOINK_RUST_API_SECRET_KEY:secret,YOINK_RUST_OWNER_ID:'42',YOINK_RUST_TELEGRAM_ENABLED:'true',YOINK_RUST_TELEGRAM_TOKEN:telegramToken,YOINK_RUST_BOT_API_URL:trusted.origin,YOINK_RUST_DOWNLOADS_ENABLED:'false',YOINK_RUST_INBOX_ENABLED:String(inbox),YOINK_RUST_INSIGHT_ENABLED:String(insight),YOINK_RUST_INSIGHT_GATEWAY_BASE:trusted.origin+'/v1',YOINK_RUST_INSIGHT_GATEWAY_KEY:'fixture-gateway',YOINK_RUST_INSIGHT_MODEL:'fixture-model',YOINK_RUST_INSIGHT_WORKSPACE:'/state/insight',YOINK_RUST_INSIGHT_CONTENT_PROXY:trusted.origin+'/',YOINK_RUST_INSIGHT_REQUEST_TIMEOUT_SECS:'20',YOINK_RUST_INSIGHT_TEXT_TIMEOUT_SECS:'5',YOINK_RUST_INSIGHT_CONTENT_TIMEOUT_SECS:'15',YOINK_RUST_INSIGHT_RESEARCH_TIMEOUT_SECS:'15',YOINK_RUST_INSIGHT_SOCKET_TIMEOUT_SECS:'10',YOINK_RUST_INSIGHT_TLDR_DAILY_LIMIT:'20',YOINK_RUST_INSIGHT_SUMMARY_DAILY_LIMIT:'50',YOINK_RUST_INSIGHT_GITHUB_CLIENT_ID:'fixture-read-client',YOINK_RUST_INSIGHT_GITHUB_PUBLIC_REPO_CLIENT_ID:'fixture-write-client',YOINK_RUST_INSIGHT_OAUTH_DEVICE_BASE:`http://127.0.0.1:${oauthPort}`,YOINK_RUST_INSIGHT_OAUTH_USER_URL:`http://127.0.0.1:${oauthPort}/user`,YOINK_RUST_INSIGHT_OAUTH_REQUEST_TIMEOUT_SECS:'10',YOINK_RUST_INSIGHT_OAUTH_USERNAME_TIMEOUT_SECS:'3',YOINK_RUST_SHUTDOWN_TIMEOUT_SECS:'90',TMPDIR:'/tmp',TURSO_TMPDIR:'/tmp'};
      if(useTls){delete env.YOINK_RUST_INSIGHT_CONTENT_PROXY;Object.assign(env,tls.env,{YOINK_FIXTURE_INSIGHT_CA_FILE:tls.nativeTrustFile});}
      // Keep public-class fixture traffic inside its owned namespace, not host policy routing.
      const networkArgs=useTls?['--network',`container:${tls.networkContainer}`,...tls.bundleMount]:['--network',network,...(dns?resolverArgs(dns):['--dns','127.0.0.1']),'--add-host',`byok.fixture.invalid:${gateway}`];
      app=await startContainer([...networkArgs,'--workdir','/state/insight','--read-only','--cap-drop','ALL','--security-opt','no-new-privileges','--pids-limit','128','--memory','512m','--tmpfs','/tmp:rw,nosuid,nodev,noexec,size=64m,mode=1777',...mount,...Object.entries(env).flatMap(([k,v])=>['--env',`${k}=${v}`]),'--entrypoint','yoink-fixture-server',config['fixture-image']]);
      appAddress=await docker(['inspect','--format',`{{(index .NetworkSettings.Networks "${network}").IPAddress}}`,useTls?tls.networkContainer:app]);assert(net.isIPv4(appAddress),'owned app IPv4');
      if(insight) {
        relay=await startContainer(['--network',`container:${app}`,'--read-only','--cap-drop','ALL','--security-opt','no-new-privileges','--pids-limit','32','--memory','64m','--entrypoint','/usr/local/bin/python',config['fixture-image'],'-I','-B','-c',relaySource,gateway,String(trusted.server.address().port),String(oauthPort)]);
        await waitFor(async()=>{try{return (await docker(['exec',app,'/usr/local/bin/python','-I','-B','-c','import socket,sys; s=socket.create_connection(("127.0.0.1",int(sys.argv[1])),1); s.close(); print("ready")',String(oauthPort)]))==='ready';}catch{return false;}},'owned OAuth loopback relay');
      }
    }
    async function ready(){let last;try{await waitFor(async()=>{check();try{const response=await fetch(`http://127.0.0.1:${httpPort}/health/ready`,{signal:AbortSignal.timeout(1000)});last=`HTTP ${response.status}: ${await response.text()}`;return response.status===200;}catch(error){last=error.cause?.message??error.message;return false;}},'native Insight readiness');}catch(error){throw new Error(`native Insight readiness: ${last}`,{cause:error});}}
    async function workspaceClean(){assert.equal(await helperContainer(['--network','none',...mount,'--entrypoint','sh',config['server-image'],'-c','test -z "$(ls -A /state/insight)" && echo clean']),'clean');}
    const processLogs=()=>app?docker(['logs',app]):Promise.resolve(stoppedLogs);
    const signalShutdown=()=>{assert(app,'original process required');return docker(['kill','--signal','TERM',app]);};
    async function confirmation(mode, token='') {
      assert(app,'live native controller required');
      const input=JSON.stringify({token:fixtureToken,sql:token,mode})+'\n';
      const result=await capture('docker',['exec','--interactive',app,'/usr/local/bin/python','-I','-B','-c',socketClient,String(fixturePort)],{timeout:10000,input});
      const response=JSON.parse(result.output.trim());
      assert.equal(response.ok,true,`native fixture control failed: ${response.error}`);
      return response;
    }
    const replayStatus=token=>confirmation('replay_status',token);
    async function finishForcedStop() {
      assert(app,'original process required');const original=app;
      assert.equal(await docker(['wait',original],{timeout:135000}),'1','original must exit with shutdown failure');
      const state=JSON.parse(await docker(['inspect','--format','{{json .State}}',original]));
      assert.equal(state.ExitCode,1);assert.equal(state.OOMKilled,false);assert.equal(state.Error,'');assert.equal(state.Running,false);
      stoppedLogs=await docker(['logs',original]);
      assert.match(stoppedLogs,/fixture database shutdown completed: result=Err\(ShutdownTimeout\)/);
      assert.match(stoppedLogs,/shutdown deadline exceeded; remaining adapter tasks were aborted/);
      if(relay){await docker(['rm','--force',relay]);relay=undefined;}
      app=undefined;appAddress=undefined;await workspaceClean();
    }
    async function stop({live=false}={}) {
      if(!app)return;const original=app;
      await docker(['kill','--signal','TERM',original]);assert.equal(await docker(['wait',original],{timeout:100000}),'0','original native process shutdown');
      const state=JSON.parse(await docker(['inspect','--format','{{json .State}}',original]));assert.equal(state.OOMKilled,false);assert.equal(state.Error,'');assert.equal(state.Running,false);
      const logs=await docker(['logs',original]);stoppedLogs=logs;assert.match(logs,/fixture graceful shutdown verified: adapters drained, worker_children=0, workspaces=0/);
      if(live){const snapshot=/fixture shutdown signal SIGTERM: worker_children=(\d+)/.exec(logs);assert(snapshot && Number(snapshot[1])>0,'original SIGTERM must observe live content child');}
      if(relay){await docker(['rm','--force',relay]);relay=undefined;}
      app=undefined;appAddress=undefined;await workspaceClean(); // stopped original remains owned until final removal
    }
    const request=api(`http://127.0.0.1:${httpPort}`),login=async actor=>{const token=(await request('/api/v1/auth/token',{method:'POST',body:{init_data:initData(actor,telegramToken)}})).access_token; secrets.push(token);return token;};
    defer(async()=>{if(app) { const name=await docker(['inspect','--format','{{.Name}}',app]); await docker(['rm','--force',app]); containers.delete(name.slice(1)); app=undefined; }});
    const startDns=()=>startHeldDns({image:config['server-image'],runId:id,poll:waitFor,
      startContainer:args=>startContainer(args.map((value,i)=>args[i-1]==='--network'?network:args[i-1]==='--name'?`yoink-insight-dns-${id}`:value)),
      docker:args=>docker(args.map(value=>value==='{{.NetworkSettings.Networks.bridge.IPAddress}}'?`{{(index .NetworkSettings.Networks "${network}").IPAddress}}`:value)),
      removeContainer:async container=>{await docker(['rm','--force',container]);},
    });
    const observeDnsWorker=async()=>{assert(app);const source=await readFile(new URL('../inbox-fixture/observe-worker.py',import.meta.url),'utf8');return JSON.parse(await docker(['exec',app,'/usr/local/bin/python','-I','-B','-c',source,'127.0.0.11']));};
    const c={config,network,tls,db,bot,workspaceClean,startDns,observeDnsWorker,trusted,request,login,launch,ready,stop,confirmation,replayStatus,processLogs,signalShutdown,finishForcedStop,httpPort,startContainer,helperContainer,docker,addSecret:value=>secrets.push(value),sanitize:error=>report(error,secrets),ensureReady:async({tls:useTls=false}={})=>{if(app && appUsesTls!==useTls)await stop();if(!app)await launch({tls:useTls});await ready();},waitFor:async(fn,label,timeout)=>waitFor(()=>{check();return fn();},label,timeout)};
    try {await execute(c);await stop();}catch(e){const logs=app?await docker(['logs',app]).catch(()=> 'logs unavailable'):stoppedLogs;const tlsEvents=tls?await tls.state().catch(()=>({events:'unavailable'})):null;const tlsLogs=tls?await tls.logs().catch(()=> 'logs unavailable'):'';throw new Error(report(e,secrets)+'\nOriginal process logs:\n'+report(logs,secrets)+'\nDenied fixture routes:\n'+report(JSON.stringify(trusted.calls.filter(call=>call.kind==='denied')),secrets)+'\nTLS events:\n'+report(JSON.stringify(tlsEvents),secrets)+'\nTLS process logs:\n'+report(tlsLogs,secrets));}
  }); } catch(e) {throw new Error(report(e,secrets));}
}
