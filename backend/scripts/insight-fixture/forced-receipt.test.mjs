import test from 'node:test';
import assert from 'node:assert/strict';
import { performance } from 'node:perf_hooks';
import {held,signalTiming,telegramTask,abortProof,shutdownProof,replayProof,finalSends,finalText,forcedReceipt,requireContext,REPLAY_BLOCKER,LIMITS} from './forced-receipt.mjs';
const token='insight:123456:44:0:39501:0:0';
const abort='fixture adapter aborted and joined: task_id=17';
test('entry and arm budgets independently preserve native hold slack',()=>{
  signalTiming({armed:0,entered:10000,signalled:15000});
  for(const times of [{armed:0,entered:1,signalled:5002},{armed:0,entered:14000,signalled:15001},{armed:10,entered:0,signalled:20}])assert.throws(()=>signalTiming(times));
});
test('hold proof rejects release, expiry, cancellation drop and wrong token',()=>{
  held({ok:true,hold:{token,state:'entered'}},token);
  for(const state of ['armed','released','expired','dropped'])assert.throws(()=>held({ok:true,hold:{token,state}},token));
  assert.throws(()=>held({ok:true,hold:{token:'other',state:'entered'}},token));
});
test('real timeout requires exact logged original Telegram task joined',()=>{
  assert.equal(telegramTask('fixture Telegram adapter task: task_id=17\n'), '17');
  assert.throws(()=>telegramTask(''));
  assert.throws(()=>telegramTask('fixture Telegram adapter task: task_id=17\nfixture Telegram adapter task: task_id=18'));
  abortProof(abort,'17',90000);
  for(const [logs,elapsed] of [[abort,89999],[abort,100001],[abort.replace('17','170'),90000],[abort+'\nfixture Insight handlers drained',90000]])assert.throws(()=>abortProof(logs,'17',elapsed));
});
test('original handler drain, concrete shutdown and DB close are ordered',()=>{
  const markers=[abort,'fixture Insight handlers drained','fixture concrete Insight shutdown completed','fixture database shutdown completed: result=Err(ShutdownTimeout)'];
  shutdownProof(markers.join('\n'),'17');
  assert.throws(()=>shutdownProof(markers.toReversed().join('\n'),'17'));
  assert.throws(()=>shutdownProof(markers.join('\n').replace('Err(ShutdownTimeout)','Ok(())'),'17'));
  assert.throws(()=>shutdownProof(markers.join('\n').replace('task_id=17','task_id=170'),'17'));
  assert.throws(()=>shutdownProof([...markers,markers[1]].join('\n'),'17'));
});
test('replay proof cannot be queue empty or unrelated live handler',()=>{
  replayProof({token,fenced:1},token);
  for(const value of [{token,fenced:0},{token:'other',fenced:1},{token,fenced:2},{pending_update_count:0}])assert.throws(()=>replayProof(value,token));
  const c=Object.fromEntries(['confirmation','processLogs','signalShutdown','finishForcedStop'].map(n=>[n,()=>{}]));
  assert.throws(()=>requireContext(c),{message:REPLAY_BLOCKER});
});
test('duplicate count is scoped to actual final send and exact source message',()=>{
  const call={method:'sendmessage',data:{chat_id:'44',reply_parameters:{message_id:39501},text:'final'}};
  assert.equal(finalSends([call,{...call,method:'editmessagetext'},{...call,data:{...call.data,chat_id:43}},{...call,data:{...call.data,reply_parameters:{message_id:39502}}}],39501).length,1);
  assert.equal(finalSends([call,call],39501).length,2);
});
const body='Native caf\u00e9 summary.';
const complete=`Complete \u00b7 tldr\n/cancel 12345678-1234-4123-8123-123456789abc\n\n${body}`;
test('cached final proof validates native header and exact body, rejecting drafts',()=>{
  finalText(complete,body);
  for(const text of [body,complete.replace('Complete','Incomplete answer'),complete+'extra',complete.replace('12345678','not-a-uuid')])assert.throws(()=>finalText(text,body));
  assert.throws(()=>finalText(complete,''));
});

// This model checks orchestration and failure assertions, not native process behavior.
function model(t, fault) {
  let now=0, state='armed', signalled=false, restarted=false, fenced=0, summaries=0, logs='fixture Telegram adapter task: task_id=17\n';
  t.mock.method(performance,'now',()=>now);
  const events=[];
  const send=(message,text=complete)=>({method:'sendmessage',data:{chat_id:44,reply_parameters:{message_id:message},text}});
  const c={
    trusted:{origin:'http://fixture.invalid',calls:[]},bot:{calls:[],enqueue(actor,chat,text,message,language){
      assert.deepEqual([actor,chat,language],text==='/start'?[45,45,'en']:[44,44,'en']);events.push(`enqueue:${message}`);
      if(text==='/start') {if(fault!=='no-witness')this.calls.push({method:'sendmessage',data:{chat_id:45,text:'live'}});return;}
      assert.equal(text,'/tldr http://93.184.216.34/article');
      if(restarted) {fenced=fault==='no-fence'?0:fault==='two-fences'?2:1;return;}
      state='entered';this.calls.push(send(message,fault==='draft'?complete.replace('Complete','Incomplete answer'):complete));
      if(fault==='provider-work')c.trusted.calls.push({kind:'completion'});
    }},
    ready:async()=>{},login:async()=> 'fixture-session',
    request:async(path,options)=>{
      assert.equal(options.token,'fixture-session');
      if(path.endsWith('/config/tldr'))return {};
      assert.equal(path,'/api/v1/insight/summarize');assert.equal(options.body.focus,undefined);
      summaries++;events.push('summarize');
      if(summaries===1)c.trusted.calls.push({kind:'content'},{kind:'completion'});
      return {cached:summaries>1 || fault==='first-cached',result:body};
    },
    db:{query:async(sql)=>{
      assert(sql.includes(token));
      if(sql.includes('count(*)'))return '0';
      if(sql.includes("state='sending'"))return state==='entered'?'1':'0';
      if(sql.includes("state='confirmed'"))return state==='released'?'1':'0';
      assert(sql.includes('state,receipt,bound'));
      return restarted && fault==='ledger-mutated'?'changed':'confirmed|receipt|bound';
    }},
    confirmation:async(mode,exact='')=>{
      events.push(mode);
      if(mode!=='confirmation_status')assert.equal(exact,token);
      if(mode==='confirmation_release') {
        if(fault==='cleanup-error')throw new Error('release failed');
        state='released';
      }
      if(mode==='confirmation_status' && signalled && fault==='expired')state='expired';
      return {ok:true,hold:{token,state}};
    },
    processLogs:async()=>{
      if(signalled && !logs.includes(abort)) {
        now+=fault==='early-abort'?89998:90000;logs+=abort+'\n';
      }
      return logs;
    },
    signalShutdown:async()=>{events.push('signal');signalled=true;now++;},
    finishForcedStop:async()=>{
      events.push('finish');assert.equal(state,'released');
      if(fault==='exit-rejected')throw new Error('original exit was not 1');
      const markers=['fixture Insight handlers drained','fixture concrete Insight shutdown completed','fixture database shutdown completed: result=Err(ShutdownTimeout)'];
      logs+=(fault==='shutdown-order'?markers.toReversed():markers).join('\n');
    },
    launch:async()=>{events.push('launch');restarted=true;fenced=fault==='dirty-counter'?1:0;},
    replayStatus:async(exact)=>{assert.equal(exact,token);events.push('replay-status');return {token:fault==='wrong-token'&&fenced?'other':token,fenced};},
    stop:async()=>{
      events.push('stop');
      if(fault==='duplicate')c.bot.calls.push(send(39501));
      if(fault==='replay-work')c.trusted.calls.push({kind:'content'});
    },
    waitFor:async(fn,label,timeout)=>{
      assert(Number.isInteger(timeout)&&timeout>0,'every wait has explicit finite bound');
      events.push(`wait:${timeout}`);
      assert(await fn(),`deadline: ${label}`);
    },
  };
  return {c,events};
}
test('forcedReceipt orchestrates genuine-cache model, held timeout, release, restart and exact replay',async t=>{
  const {c,events}=model(t);
  await forcedReceipt(c);
  assert.deepEqual(events.filter(e=>['summarize','signal','finish','launch','stop'].includes(e)),['summarize','summarize','signal','finish','launch','stop']);
  assert(events.indexOf('confirmation_release')<events.indexOf('finish'));
  assert(events.indexOf('replay-status')<events.indexOf('enqueue:39501',events.indexOf('launch')));
  assert(events.includes(`wait:${LIMITS.abortObservation}`));
  assert.equal(events.filter(e=>e===`wait:${LIMITS.replayObservation}`).length,2);
});
test('forcedReceipt fails closed on modeled acceptance failures',async t=>{
  const failures={
    'first-cached':/genuinely primes cache/,'draft':/completed Telegram header/,'provider-work':/native HTTP cache/,
    'early-abort':/real 90s/,'expired':/exact native hold/,'exit-rejected':/original exit/,
    'shutdown-order':/ordered original shutdown/,'dirty-counter':/reset in restarted process/,
    'no-witness':/live \/start/,'no-fence':/existing-ledger fence/,'two-fences':/existing-ledger fence/,
    'wrong-token':/existing-ledger fence/,'ledger-mutated':/cannot mutate/,'duplicate':/duplicate final/,'replay-work':/cannot start product work/,
  };
  for(const [fault,error] of Object.entries(failures))await t.test(fault,async child=>{
    const {c}=model(child,fault);await assert.rejects(forcedReceipt(c),error);
  });
});
test('forcedReceipt preserves primary assertion and hold cleanup errors',async t=>{
  const {c}=model(t,'cleanup-error');
  c.bot.enqueue=()=>{throw new Error('enqueue failed');};
  await assert.rejects(forcedReceipt(c),error=>{
    assert(error instanceof AggregateError);
    assert.deepEqual(error.errors.map(e=>e.message),['enqueue failed','release failed']);return true;
  });
});
test('forcedReceipt missing exact observation fails before any fixture operation',async()=>{
  const c=Object.fromEntries(['confirmation','processLogs','signalShutdown','finishForcedStop'].map(name=>[name,()=>{throw new Error('must not run');}]));
  await assert.rejects(forcedReceipt(c),{message:REPLAY_BLOCKER});
});
