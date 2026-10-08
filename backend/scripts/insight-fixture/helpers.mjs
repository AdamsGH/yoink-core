import assert from 'node:assert/strict';
export { api, freePort, initData, waitFor, literal } from '../inbox-fixture/helpers.mjs';
export const GATES = ['forcedReceipt', 'routing', 'identity', 'management', 'policy', 'byok', 'summary', 'cacheIsolation', 'failures', 'telegram', 'cancellation', 'revocation', 'heldRevocation', 'telegramRevocation', 'oauth', 'oauthStart', 'abortedQuota', 'receiptShutdown', 'genericReplyShutdown', 'shutdown', 'heldDns', 'positiveByok', 'positiveCacheIsolation', 'positiveYoutube', 'positiveConfinement', 'spa'];
export const GAPS = [];
export function options(args) {
  const out = {};
  for (let i = 0; i < args.length; i += 2) {
    const key = args[i]?.replace(/^--/, '');
    assert(args[i]?.startsWith('--') && ['server-image','fixture-image','worker-image','frontend-image','chromium','scenarios'].includes(key), 'unknown option');
    assert(args[i+1] && !args[i+1].startsWith('--') && !Object.hasOwn(out,key), 'missing/repeated option');
    out[key] = args[i+1];
  }
  for (const key of ['server-image','fixture-image','worker-image']) assert(out[key], `required --${key}`);
  for (const [key,value] of Object.entries(out)) if (key.endsWith('image')) assert(/^(sha256:[a-f0-9]{64}|[A-Za-z0-9][A-Za-z0-9._:/-]*@sha256:[a-f0-9]{64})$/.test(value), 'immutable image required');
  out.selected = out.scenarios?.split(',') ?? GATES;
  assert(out.selected.length && new Set(out.selected).size === out.selected.length && out.selected.every(x=>GATES.includes(x)), 'invalid gates');
  if (out.chromium) assert(out.chromium.startsWith('/') && !out.chromium.includes('\0'), 'absolute chromium path required');
  return out;
}
// Preserve primary failure AND cleanup failures; never replace a failed gate with
// successful forced removal. Ownership registration precedes resource setup.
export async function owned(execute) {
  const actions = []; let value, primary;
  try { value = await execute(action => { assert.equal(typeof action,'function'); actions.push(action); }); }
  catch (error) { primary = error; }
  const failures = [];
  for (const action of actions.reverse()) try { await action(); } catch (error) { failures.push(error); }
  if (primary || failures.length) throw new AggregateError([...(primary ? [primary] : []), ...failures], 'fixture execution/cleanup failed');
  return value;
}
export function report(error, secrets = []) {
  const seen = new Set();
  function flatten(e) { if (seen.has(e)) return ''; seen.add(e); return [e?.stack ?? String(e), ...(e?.errors ?? []).map(flatten), ...(e?.cause ? [flatten(e.cause)] : [])].join('\n'); }
  let text = flatten(error);
  for (const secret of secrets.filter(Boolean)) text = text.replaceAll(secret,'[REDACTED]');
  return text.slice(-12000);
}
