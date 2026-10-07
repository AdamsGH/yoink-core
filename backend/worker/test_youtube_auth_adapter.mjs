import { test } from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import { spawn } from 'node:child_process';
import { mkdtemp, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { once } from 'node:events';

function call(socket, path, body) {
  return new Promise((resolve, reject) => {
    const req = http.request({ socketPath: socket, method:'POST', path, headers:{'Content-Type':'application/json'} }, res => {
      const chunks=[]; res.on('data', c=>chunks.push(c)); res.on('end',()=>resolve({status:res.statusCode,body:Buffer.concat(chunks).toString()}));
    }); req.on('error',reject); req.end(JSON.stringify(body));
  });
}

test('confined gateway executes fixed routes, never redirects or exposes refreshed credentials', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'yoink-auth-'));
  const socket = join(dir,'adapter.sock');
  const requests=[];
  const fixture=http.createServer(async (req,res)=>{
    let body=''; for await(const c of req)body+=c;
    requests.push({url:req.url,method:req.method,body});
    if(body.includes('redirect')) {res.writeHead(302,{Location:'http://127.0.0.1:1/secret'}).end();return;}
    if(body.includes('refresh')) {res.writeHead(200,{'x-updated-tokens':'must-not-escape'}).end('secret');return;}
    res.end(req.url==='/download'?'synthetic-media':JSON.stringify({title:'Fixture',version:'2.0.0',poToken:'synthetic-pot'}));
  });
  fixture.listen(0,'127.0.0.1'); await once(fixture,'listening');
  const upstream=`http://127.0.0.1:${fixture.address().port}`;
  const process=spawn('node',[new URL('./youtube_auth_adapter.mjs',import.meta.url).pathname],{env:{PATH:globalThis.process.env.PATH,YOINK_YOUTUBE_AUTH_SOCKET:socket,YOINK_TRUSTED_YOUTUBEI_URL:upstream,YOINK_TRUSTED_BGUTIL_URL:upstream},stdio:['ignore','pipe','pipe']});
  let logs='';process.stdout.on('data',c=>logs+=c);process.stderr.on('data',c=>logs+=c);
  try {
    for(let n=0;n<100;n++) {try {await stat(socket);break;}catch{await new Promise(r=>setTimeout(r,20));}}
    assert.equal((await stat(socket)).mode&0o777,0o600);
    assert.deepEqual(await call(socket,'/download',{tokens:{access_token:'synthetic'}}),{status:200,body:'synthetic-media'});
    assert.equal((await call(socket,'/info',{})).status,200);
    assert.equal(requests.at(-1).method,'POST');
    assert.equal((await call(socket,'/pot/ping',{})).status,200);
    assert.equal(requests.at(-1).url,'/ping');
    assert.equal((await call(socket,'/pot',{proxy:'http://private/'})).status,502);
    assert.equal((await call(socket,'/unregistered',{})).status,404);
    assert.equal((await call(socket,'/download',{redirect:true})).status,502);
    assert.equal((await call(socket,'/download',{refresh:true})).status,502);
    assert.equal((await call(socket,'/info',{padding:'x'.repeat(65537)})).status,502);
    assert.equal(logs,'');
  } finally {
    process.kill('SIGTERM');await once(process,'exit');fixture.close();await rm(dir,{recursive:true,force:true});
  }
});
