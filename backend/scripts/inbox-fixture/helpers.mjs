import assert from 'node:assert/strict';
import net from 'node:net';
import { createHmac } from 'node:crypto';
import { setTimeout as sleep } from 'node:timers/promises';

export const SCENARIOS = ['routing', 'httpContracts', 'telegramContracts', 'groupContracts', 'shutdownContracts', 'deadlineContracts', 'storageContracts', 'importContracts', 'spaContracts'];
export function options(args) {
  const result = {};
  for (let i = 0; i < args.length; i += 2) {
    const key = args[i];
    assert(['--server-image', '--fixture-image', '--worker-image', '--operator-image', '--importer-image', '--frontend-image', '--chromium', '--scenarios'].includes(key), `unknown argument ${key}`);
    assert(args[i + 1] && !args[i + 1].startsWith('--') && !result[key.slice(2)], `missing/repeated ${key}`);
    result[key.slice(2)] = args[i + 1];
  }
  for (const key of ['server-image', 'fixture-image', 'operator-image']) assert(result[key], `required --${key}`);
  for (const [key, value] of Object.entries(result)) {
    if (key.endsWith('image')) assert(/^(?:sha256:[a-f0-9]{64}|[A-Za-z0-9][A-Za-z0-9._:/-]*@sha256:[a-f0-9]{64})$/.test(value), `${key} must be an immutable image ID/digest`);
  }
  if (result.scenarios) {
    const selected = result.scenarios.split(',');
    assert(selected.length > 0 && new Set(selected).size === selected.length && selected.every(name => SCENARIOS.includes(name)), 'invalid/repeated scenarios');
  }
  result['worker-image'] ??= result['server-image'];
  if (result.chromium) assert(result.chromium.startsWith('/') && !result.chromium.includes('\0'), 'chromium must be an absolute executable path');
  return result;
}
export async function waitFor(fn, label, timeout = 30000) {
  assert(typeof fn === 'function' && typeof label === 'string' && label.length > 0 && label.length <= 256, 'invalid poll inputs');
  assert(Number.isInteger(timeout) && timeout >= 1 && timeout <= 180000, 'invalid poll deadline');
  const end = Date.now() + timeout;
  do { const value = await fn(); if (value) return value; await sleep(100); } while (Date.now() < end);
  throw new Error(`deadline: ${label}`);
}
export async function freePort() {
  const server = net.createServer();
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  const port = server.address().port;
  await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  return port;
}
export function initData(actor, token, now = Math.floor(Date.now() / 1000)) {
  assert(Number.isSafeInteger(actor) && actor > 0);
  assert(typeof token === 'string' && token.length > 0);
  const user = JSON.stringify({ id: actor, first_name: 'Inbox Fixture', username: `fixture_${actor}` });
  const auth_date = String(now);
  const secret = createHmac('sha256', 'WebAppData').update(token).digest();
  const hash = createHmac('sha256', secret).update(`auth_date=${auth_date}\nuser=${user}`).digest('hex');
  return new URLSearchParams({ auth_date, user, hash }).toString();
}
export const literal = value => `'${String(value).replaceAll("'", "''")}'`;
export async function cleanup(actions) {
  const errors = [];
  for (const action of actions.reverse()) { try { await action(); } catch { errors.push('owned resource cleanup failed'); } }
  if (errors.length) throw new AggregateError(errors.map(x => new Error(x)), 'fixture cleanup incomplete');
}
export function report(error, secrets = []) {
  let text = String(error?.stack ?? error);
  for (const secret of secrets.filter(Boolean)) text = text.replaceAll(secret, '[REDACTED]');
  return text.slice(-12000);
}
export function api(base) {
  return async (path, { token, method = 'GET', body, expected = 200, headers = {} } = {}) => {
    const response = await fetch(base + path, { method, signal: AbortSignal.timeout(7000), headers: {
      ...headers, ...(token ? { authorization: `Bearer ${token}` } : {}), ...(body === undefined ? {} : { 'content-type': 'application/json' }),
    }, ...(body === undefined ? {} : { body: JSON.stringify(body) }) });
    let size = 0;
    const chunks = [];
    if (response.body) {
      const reader = response.body.getReader();
      try {
        for (;;) {
          const { done, value } = await reader.read(); if (done) break;
          size += value.length;
          if (size > 1024 * 1024) { await reader.cancel(); throw new Error('HTTP response exceeds fixture bound'); }
          chunks.push(value);
        }
      } finally { reader.releaseLock(); }
    }
    const text = Buffer.concat(chunks).toString('utf8');
    assert.equal(response.status, expected, `${method} ${path}: unexpected HTTP status`);
    return text ? JSON.parse(text) : null;
  };
}
