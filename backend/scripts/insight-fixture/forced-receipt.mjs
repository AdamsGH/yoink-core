import assert from 'node:assert/strict';
import { performance } from 'node:perf_hooks';
import { ARTICLE } from './services.mjs';
import { configure, session } from './scenarios.mjs';

export const LIMITS = Object.freeze({ entrySignal: 5000, armSignal: 15000, shutdown: 90000, abortObservation: 100000, hold: 120000, replayObservation: 30000 });
export const REPLAY_BLOCKER = 'Missing fixture-only c.replayStatus(token): process-local observation that the exact replay handler reached its existing-ledger fence; /start and getUpdates offsets do not prove concurrent /tldr replay ran.';
export function requireContext(c) {
  for (const name of ['confirmation','processLogs','signalShutdown','finishForcedStop']) assert.equal(typeof c[name], 'function', `coordinator context requires c.${name}()`);
  assert(typeof c.replayStatus === 'function', REPLAY_BLOCKER);
}
export function held(response, token, state='entered') {
  assert.equal(response.ok, true);
  assert.deepEqual(response.hold, {token, state}, 'exact native hold must remain entered, unreleased, unexpired and not dropped');
}
export function signalTiming({armed, entered, signalled}) {
  assert(armed <= entered && entered <= signalled, 'monotonic observation ordering');
  assert(signalled-entered <= LIMITS.entrySignal, 'entry-to-signal bound');
  assert(signalled-armed <= LIMITS.armSignal, 'arm-to-signal leaves room for real 90s timeout within 120s hold');
}
export function telegramTask(logs) {
  const matches=[...logs.matchAll(/fixture Telegram adapter task: task_id=(\d+)\b/g)];
  assert.equal(matches.length,1,'one original Telegram task identity');
  return matches[0][1];
}
export function abortProof(logs, task, elapsed) {
  assert(elapsed >= LIMITS.shutdown, 'must observe the real 90s shutdown timeout, not early cancellation');
  assert(elapsed <= LIMITS.abortObservation, 'bounded abort observation');
  const marker=`fixture adapter aborted and joined: task_id=${task}`;
  assert(logs.split('\n').some(line=>line.trim()===marker), 'the exact Telegram adapter was aborted AND joined');
  assert(!logs.includes('fixture Insight handlers drained'), 'held original handler must not have drained before release');
}
export function shutdownProof(logs, task) {
  const markers=[`fixture adapter aborted and joined: task_id=${task}`,'fixture Insight handlers drained','fixture concrete Insight shutdown completed','fixture database shutdown completed: result=Err(ShutdownTimeout)'];
  const lines=logs.split('\n').map(line=>line.trim());
  let position=-1;
  for(const marker of markers) {assert.equal(lines.filter(line=>line===marker).length,1,`one exact shutdown marker: ${marker}`);const next=lines.indexOf(marker);assert(next>position,`ordered original shutdown marker: ${marker}`);position=next;}
}
export function replayProof(value, token) {
  assert.equal(value.token,token);assert.equal(value.fenced,1,'exact replay processed at ledger fence once in restarted process');
}
export function finalSends(calls, message) {
  return calls.filter(x=>x.method==='sendmessage' && Number(x.data.chat_id)===44 && Number(x.data.reply_parameters?.message_id)===message);
}
export function finalText(text, result) {
  assert.equal(typeof result,'string');assert(result.length>0,'nonempty genuine cached result');
  const header=/^Complete \u00b7 tldr\n\n/.exec(text);
  assert(header,'native completed Telegram header, not an incremental draft');
  assert.equal(text.slice(header[0].length),result,'unchanged same-user cached HTTP body');
}

// No shortened production budgets, held HTTP receipt, synthetic effect or SQL seed.
// Requires the fixture-only replay observation described in forced-receipt-notes.md.
export async function forcedReceipt(c) {
  requireContext(c);
  await configure(c);
  const user=await session(c,44), message=39501, token=`insight:123456:44:0:${message}:0:0`;
  const body={command:'tldr',url:ARTICLE};
  assert.equal(await c.db.query(`SELECT count(*) FROM runtime_text_effects WHERE token='${token}'`),'0');
  const first=await user('/summarize',{method:'POST',body});
  const cached=await user('/summarize',{method:'POST',body});
  assert.equal(first.cached,false,'first same-user HTTP request genuinely primes cache');
  assert.equal(cached.cached,true,'real native HTTP cache primed for same identity');assert.equal(cached.result,first.result);
  const providers=c.trusted.calls.length, before=c.bot.calls.length;
  const armed=performance.now();held(await c.confirmation('confirmation_arm',token),token,'armed');
  let released=false, primary;
  try {
    c.bot.enqueue(44,44,`/tldr ${ARTICLE}`,message,'en');
    await c.waitFor(async()=>{const s=await c.confirmation('confirmation_status');return s.ok && s.hold?.token===token && s.hold.state==='entered';},'native validated final receipt enters exact confirmation hold',10000);
    const entered=performance.now();
    assert.equal(await c.db.query(`SELECT state='sending' AND receipt IS NULL FROM runtime_text_effects WHERE token='${token}'`),'1');
    const sends=finalSends(c.bot.calls.slice(before),message);assert.equal(sends.length,1);finalText(sends[0].data.text,cached.result);
    assert(!c.trusted.calls.slice(providers).some(x=>['content','completion'].includes(x.kind)),'TG final came from native HTTP cache, not stream/draft');
    const task=telegramTask(await c.processLogs());
    held(await c.confirmation('confirmation_status'),token);
    const signalStart=performance.now();await c.signalShutdown();const signalled=performance.now();
    signalTiming({armed,entered,signalled});
    await c.waitFor(async()=>{const logs=await c.processLogs();return logs.split('\n').some(line=>line.trim()===`fixture adapter aborted and joined: task_id=${task}`);},'original Telegram adapter aborted and joined at real shutdown timeout',LIMITS.abortObservation);
    abortProof(await c.processLogs(),task,performance.now()-signalStart);
    held(await c.confirmation('confirmation_status'),token);
    assert(performance.now()-armed<LIMITS.hold,'native hold has not expired');
    held(await c.confirmation('confirmation_release',token),token,'released');released=true;
    await c.finishForcedStop(); // asserts original exit 1, ShutdownTimeout, no OOM/SIGKILL; detaches original
    shutdownProof(await c.processLogs(),task);
    const sql=`SELECT state,receipt,bound FROM runtime_text_effects WHERE token='${token}'`;
    const stopped=await c.db.query(sql);
    assert.equal(await c.db.query(`SELECT state='confirmed' AND receipt IS NOT NULL FROM runtime_text_effects WHERE token='${token}'`),'1','released owned handler confirmed validated receipt before DB close');
    assert.equal(finalSends(c.bot.calls.slice(before),message).length,1);
    await c.launch();await c.ready();
    const initial=await c.replayStatus(token);assert.equal(initial.token,token);assert.equal(initial.fenced,0,'observation must be reset in restarted process');
    const replayStart=c.bot.calls.length;
    c.bot.enqueue(44,44,`/tldr ${ARTICLE}`,message,'en');
    c.bot.enqueue(45,45,'/start',message+1,'en');
    await c.waitFor(()=>c.bot.calls.slice(replayStart).some(x=>x.method==='sendmessage' && Number(x.data.chat_id)===45),'separate live /start handler response',LIMITS.replayObservation);
    await c.waitFor(async()=>{const s=await c.replayStatus(token);return s.token===token && s.fenced===1;},'exact concurrent replay handler observed at existing-ledger fence',LIMITS.replayObservation);
    replayProof(await c.replayStatus(token),token);
    await c.stop(); // drain all replay handlers before negative assertions
    assert.equal(await c.db.query(sql),stopped,'replay cannot mutate stopped receipt');
    assert.equal(finalSends(c.bot.calls.slice(before),message).length,1,'no unsafe duplicate final send');
    assert(!c.trusted.calls.slice(providers).some(x=>['content','completion'].includes(x.kind)),'fenced replay cannot start product work');
  } catch(error) {
    primary=error;throw error;
  } finally {
    if(!released) {
      try {held(await c.confirmation('confirmation_release',token),token,'released');}
      catch(error) {throw new AggregateError(primary?[primary,error]:[error],'forced receipt gate/hold cleanup failed');}
    }
  }
}
