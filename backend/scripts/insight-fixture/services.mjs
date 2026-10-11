import http from 'node:http';
import assert from 'node:assert/strict';
export const ARTICLE = 'http://93.184.216.34/article';
export const CONTENT = 'Confined fixture article: native full-content extraction must pass this sentence to the provider.';
export class Services {
  calls = []; sockets = new Set(); held = new Set(); holdContent = false; holdProvider = false; holdStream = false; providers = new Map();
  contentFailure = false; providerFailure = false; markdown = false; oauthPlans = []; flows = new Map(); polls = new Map(); starts = new Map(); receipts = new Map(); holdReceipt = false; oauthSequence = 0;
  releasePolls() { for(const release of [...this.polls.values()])release(); }
  releaseStarts() { for(const release of [...this.starts.values()])release(); }
  releaseReceipts() { for(const release of [...this.receipts.values()])release(); }
  json(res,value,status=200) {res.writeHead(status,{'content-type':'application/json'});res.end(JSON.stringify(value));}
  async hold(map,res) {if(res.destroyed)return;await new Promise(resolve=>{const release=()=>{map.delete(res);resolve();};map.set(res,release);res.once('close',release);});}
  async read(req) {let raw='';for await(const chunk of req){raw+=chunk;assert(Buffer.byteLength(raw)<=128*1024);}return JSON.parse(raw);}
  async oauth(req,res) {
    if(req.method==='POST' && req.url==='/login/device/code') {
      const body=await this.read(req);assert(['fixture-read-client','fixture-write-client'].includes(body.client_id));
      assert.equal(body.scope,body.client_id==='fixture-write-client'?'public_repo':'read:user');
      const code=`device-${++this.oauthSequence}`,plan=this.oauthPlans.shift()??{mode:'success'};
      this.flows.set(code,{...plan,write:body.client_id==='fixture-write-client',client:body.client_id});this.calls.push({kind:'oauth-start',code,scope:body.scope});
      if(plan.mode==='holdStart')await this.hold(this.starts,res);
      if(res.destroyed)return true;
      if(plan.mode==='malformed')this.json(res,{device_code:code,user_code:42});
      else this.json(res,{device_code:code,user_code:`FIXTURE-${code.slice(7)}`,expires_in:plan.expires??60,interval:1});return true;
    }
    if(req.method==='POST' && req.url==='/login/oauth/access_token') {
      const body=await this.read(req),flow=this.flows.get(body.device_code);assert(flow);assert.equal(body.client_id,flow.client);assert.equal(body.grant_type,'urn:ietf:params:oauth:grant-type:device_code');
      this.calls.push({kind:'oauth-poll',code:body.device_code});
      if(flow.mode==='hold')await this.hold(this.polls,res);
      if(res.destroyed)return true;
      if(flow.mode==='expired')this.json(res,{error:'expired_token'});
      else if(flow.mode==='pending')this.json(res,{error:'authorization_pending'});
      else if(flow.mode==='denied')this.json(res,{error:'access_denied'});
      else this.json(res,{access_token:flow.token??(flow.write?'fixture-oauth-write':'fixture-oauth-read'),scope:flow.mode==='missingScope'?'read:user':flow.write?'public_repo':'read:user'});
      return true;
    }
    if(req.method==='GET' && req.url==='/user') {
      assert(['token fixture-oauth-read','token fixture-oauth-write','token fixture-oauth-late'].includes(req.headers.authorization));this.calls.push({kind:'oauth-user'});this.json(res,{login:'fixture-user'});return true;
    }
    return false;
  }
  releaseProviders() { for(const release of [...this.providers.values()])release(); }
  async start(host = '127.0.0.1') {
    this.server = http.createServer((req,res)=>void this.handle(req,res).catch(()=>{res.writeHead(400);res.end();}));
    this.server.on('connection',s=>{this.sockets.add(s);s.once('close',()=>this.sockets.delete(s));});
    const tunnel=http.createServer((req,res)=>{
      if(req.method==='GET' && req.headers.host==='93.184.216.34' && req.url==='/llms.txt'){res.writeHead(404);res.end();return;}
      if(req.method!=='GET' || req.headers.host!=='93.184.216.34' || req.url!=='/article'){res.writeHead(403);res.end();return;}
      req.url=ARTICLE;void this.handle(req,res).catch(()=>{res.writeHead(400);res.end();});
    });
    this.server.on('connect',(req,s,head)=>{
      if(req.url!=='93.184.216.34:80'){s.end('HTTP/1.1 403 Forbidden\r\n\r\n');return;}
      s.write('HTTP/1.1 200 Connection Established\r\n\r\n');
      if(head.length)s.unshift(head);tunnel.emit('connection',s);
    });
    await new Promise((resolve,reject)=>{this.server.once('error',reject);this.server.listen(0,host,resolve);});
    this.origin = `http://${host}:${this.server.address().port}`; return this;
  }
  async handle(req,res) {
    // Sole forwarding exception is the local public BotApiFixture helper.
    if (this.botOrigin && req.url.startsWith('/bot123456:fixture_insight_token/') && /^\/bot123456:fixture_insight_token\/(getMe|getUpdates|setMyCommands|deleteMyCommands|getMyCommands|sendMessage|sendMessageDraft|sendChatAction|editMessageText|deleteMessage|answerCallbackQuery)$/i.test(req.url) && ['GET','POST'].includes(req.method)) {
      const stall=this.holdReceipt && /\/(sendMessage|editMessageText)$/i.test(req.url);
      const upstream=http.request(this.botOrigin+req.url,{method:req.method,headers:req.headers},reply=>{
        if(!stall){res.writeHead(reply.statusCode,reply.headers);reply.pipe(res);return;}
        // The owned Bot API accepted the effect; retain its exact receipt, not a
        // fabricated response. Native ledger admission is checked by the gate.
        const chunks=[];let size=0;reply.on('data',chunk=>{size+=chunk.length;if(size>65536){reply.destroy();res.destroy();return;}chunks.push(chunk);});
        reply.once('end',()=>{void (async()=>{await this.hold(this.receipts,res);if(!res.destroyed){res.writeHead(reply.statusCode,reply.headers);res.end(Buffer.concat(chunks));}})();});
      });upstream.once('error',()=>{res.writeHead(502);res.end();});req.pipe(upstream);res.once('close',()=>upstream.destroy());return;
    }
    if(await this.oauth(req,res))return;
    // No external upstream forwarding, DNS resolution or unknown provider fallback.
    if (req.method === 'GET' && req.url === ARTICLE) {
      this.calls.push({kind:'content'});
      if(this.contentFailure){res.writeHead(503);res.end('fixture content failure');return;}
      if (this.holdContent) { this.held.add(res); res.once('close',()=>this.held.delete(res)); return; }
      res.setHeader('content-type','text/html');res.end(`<html><head><title>Fixture</title></head><body><article><p>${CONTENT}</p><p>${CONTENT}</p></article></body></html>`); return;
    }
    if(req.method==='GET' && req.url==='/v1/models' && ['Bearer fixture-gateway','Bearer fixture-gateway-two'].includes(req.headers.authorization)) {this.calls.push({kind:'models'});res.setHeader('content-type','application/json');res.end(JSON.stringify({data:[{id:'fixture-model'}]}));return;}
    if (req.method !== 'POST' || req.url !== '/v1/chat/completions' || !['Bearer fixture-gateway','Bearer fixture-gateway-two'].includes(req.headers.authorization)) { this.calls.push({kind:'denied',path:req.url});res.writeHead(403);res.end();return; }
    let raw = ''; for await (const chunk of req) { raw += chunk; assert(Buffer.byteLength(raw)<=128*1024); }
    const body = JSON.parse(raw); assert.equal(body.model,'fixture-model');
    assert(JSON.stringify(body.messages).includes(CONTENT), 'actual worker content must reach actual completion adapter');
    const prompt = JSON.stringify(body.messages);
    assert(prompt.includes(`Current UTC date: ${new Date().toISOString().slice(0,10)}`));
    assert(prompt.includes('Your training memory is not independent verification'));
    assert(prompt.includes('say the claim is unverified, not false'));
    this.calls.push({kind:'completion',stream:body.stream===true,credential:req.headers.authorization.endsWith('-two')?'two':'one'});
    if(this.providerFailure){this.json(res,{error:{message:'fixture provider unavailable'}},503);return;}
    if(this.holdProvider)await this.hold(this.providers,res);
    if(res.destroyed)return;
    const generated = this.markdown ? [
      '# Fixture', '', 'Native **café** summary.', '',
      '😀 **decision — verified**. *context* and ~~discarded~~.', '',
      'A 6" screen — assessment. The \'90s — context.', '',
      '"quoted — [ending"](https://example.com) prose — assessment', '',
      '- **First** `token`.', '- Second', '  - nested', '- Third', '',
      '| Field | Value |', '| --- | --- |', '| **range** | `1–2` [literal—label](https://example.com/table) |', '',
      '> Quoted evidence — literal.', '', 'Use `token—literal`.', '',
      '```rust', 'let x = "—";', '```',
    ].join('\n') : 'Native café summary.';
    if (body.stream) {
      res.setHeader('content-type','text/event-stream');
      if(this.holdStream) {res.write('data: '+JSON.stringify({choices:[{delta:{content:'Draft only.'}}]})+'\n\n');await this.hold(this.providers,res);if(res.destroyed)return;}
      // Deliberately cut UTF-8 and SSE JSON across writes. The summary assertion
      // proves decoding only when the native adapter actually requested streaming.
      const bytes=Buffer.from('data: '+JSON.stringify({choices:[{delta:{content:generated}}]})+'\n\ndata: [DONE]\n\n');
      const cuts=[...new Set([...Array.from({length:Math.ceil(bytes.length/3)},(_,i)=>Math.min((i+1)*3,bytes.length)),bytes.indexOf(Buffer.from('é'))+1])].sort((a,b)=>a-b);
      let start=0;for (const end of cuts) { if (res.destroyed) return;res.write(bytes.subarray(start,end));start=end;await new Promise(r=>setTimeout(r,1)); }res.end();
    } else {res.setHeader('content-type','application/json');res.end(JSON.stringify({choices:[{message:{content:generated}}]}));}
  }
  async close() {
    const closed=[...this.sockets].map(socket=>new Promise(resolve=>{socket.once('close',resolve);socket.destroy();}));
    if(this.server?.listening)closed.push(new Promise((resolve,reject)=>this.server.close(e=>e?reject(e):resolve())));
    await Promise.all(closed);
  }
}
