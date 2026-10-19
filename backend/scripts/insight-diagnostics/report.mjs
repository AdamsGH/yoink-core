const stages = new Set([
  'received','phase','preview','provider','generated','sending','delivered',
  'delivery_unconfirmed','completed','generation_failed','final_failed',
]);
const strings = new Set([
  'tldr','summary','about','OpenAi','Anthropic','Gemini',
  'gateway','openrouter','byok','byok_openai','byok_openrouter','byok_perplexity','byok_anthropic','byok_gemini',
  'Fetching','Researching','Generating','Cached','Sent','Unsupported','Unavailable',
  'ok','stop','done','length','max_tokens','end_turn','stop_sequence','other','absent','timeout','cancelled',
  'response_truncated','stream_incomplete','provider_unavailable','parse_failed','llm_error',
  'content_blocked','empty_response','response_too_large','database_error','outbound_unknown',
  'access_denied','route_changed','auth_failed','model_not_found','no_route','quota_exceeded',
  'sent','unsupported','invalid_draft','transport_error','invalid_response','api_error',
]);
const fields = new Set([
  'command','input_characters','brief_requested','reviewer_requested','phase','outcome',
  'text_bytes','attempt','shape','route','end_reason','wire_bytes','deltas','elapsed_ms',
  'first_delta_ms','cached','part','final','receipt','edit','error','reason','http_status','api_error_code',
]);

export const isRequestId = value => typeof value === 'string' && /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(value);

export function parseEvents(logs, request) {
  const events = [];
  for (const line of logs.split('\n')) {
    const match = /insight_diagnostic=(\{.*\})/.exec(line);
    if (!match) continue;
    let value;
    try { value = JSON.parse(match[1]); } catch { continue; }
    if (value.version !== 1 || !isRequestId(value.request_id) || !stages.has(value.stage) || (request && value.request_id !== request)) continue;
    if (!Number.isSafeInteger(value.chat_id) || !Number.isSafeInteger(value.input_message_id)) continue;
    const details = {};
    for (const [key,item] of Object.entries(value.details ?? {})) {
      if (!fields.has(key)) continue;
      if (typeof item === 'string') details[key] = strings.has(item) ? item : 'other';
      else if (typeof item === 'boolean' || item === null || (Number.isSafeInteger(item) && item >= 0)) details[key] = item;
    }
    const timestamp = /^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d[.\d]*Z)/.exec(line)?.[1];
    events.push({timestamp,request_id:value.request_id,chat_id:value.chat_id,input_message_id:value.input_message_id,stage:value.stage,details});
  }
  return events.sort((a,b) => (a.timestamp ?? '').localeCompare(b.timestamp ?? ''));
}

export function summarize(events) {
  const requests = new Map();
  for (const event of events) {
    if (!requests.has(event.request_id)) requests.set(event.request_id, {request_id:event.request_id,chat_id:event.chat_id,input_message_id:event.input_message_id,events:[]});
    requests.get(event.request_id).events.push(event);
  }
  return [...requests.values()].map(request => {
    const has = stage => request.events.some(event => event.stage === stage);
    request.classification = has('delivery_unconfirmed') ? 'final_delivery_unconfirmed'
      : has('final_failed') ? 'final_delivery_failed'
      : request.events.some(event => event.details.error === 'response_truncated' || event.details.outcome === 'response_truncated') ? 'generation_truncated'
      : has('generation_failed') ? 'generation_failed'
      : has('completed') ? 'completed' : 'in_progress_or_missing_tail';
    return request;
  });
}
