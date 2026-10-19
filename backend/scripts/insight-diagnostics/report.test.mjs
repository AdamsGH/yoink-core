import test from 'node:test';
import assert from 'node:assert/strict';
import {isRequestId,parseEvents,summarize} from './report.mjs';
const id='00000000-0000-4000-8000-000000000001';
const line = (stage,details={}) => '2026-10-13T08:00:00Z INFO insight_diagnostic='+JSON.stringify({version:1,request_id:id,chat_id:1,input_message_id:2,stage,details});
test('reports reject malformed events and omit text, secrets and unknown values',()=>{
  const result=parseEvents(line('provider',{outcome:'ok',route:'openrouter',text_bytes:30,token:'SENTINEL',text:'SENTINEL',error:'SENTINEL'})+'\n'+line('secret',{outcome:'SENTINEL'})+'\ninsight_diagnostic={invalid}');
  assert.equal(result.length,1);assert.equal(result[0].details.text_bytes,30);assert(!JSON.stringify(result).includes('SENTINEL'));
  assert.equal(parseEvents(line('received'),'00000000-0000-4000-8000-000000000002').length,0);
  assert.equal(result[0].details.route,'openrouter');assert(isRequestId(id));assert(!isRequestId('-'.repeat(36)));
});
test('separates truncated generation, final failure, completion and missing tail',()=>{
  for(const [stage,details,classification] of [['provider',{outcome:'response_truncated'},'generation_truncated'],['final_failed',{error:'database_error'},'final_delivery_failed'],['delivery_unconfirmed',{},'final_delivery_unconfirmed'],['completed',{},'completed'],['received',{},'in_progress_or_missing_tail']]) assert.equal(summarize(parseEvents(line(stage,details)))[0].classification,classification);
});
