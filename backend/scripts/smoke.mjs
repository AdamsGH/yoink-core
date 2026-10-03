// Process-level checks use only ephemeral loopback listeners and fake credentials.
import assert from 'node:assert/strict';
import { execFile, spawn } from 'node:child_process';
import { createHmac } from 'node:crypto';
import { chmod, mkdtemp, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import { once } from 'node:events';
import http from 'node:http';
import net from 'node:net';
import { setTimeout as sleep } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { CommandMenus } from './media-fixture/command-menus.mjs';

const binary = fileURLToPath(new URL('../target/debug/yoink-server', import.meta.url));
const fakeToken = '123456:smoke-secret-never-log';
const fakeMe = { id: 123456, is_bot: true, first_name: 'Fixture', username: 'yoink_fixture_bot', can_join_groups: false, can_read_all_group_messages: false, supports_inline_queries: false, has_main_web_app: false };
const fetch = (url, options = {}) => globalThis.fetch(url, { ...options, signal: AbortSignal.timeout(2000) });

async function until(check, label, milliseconds = 5000) {
  const deadline = Date.now() + milliseconds;
  while (Date.now() < deadline) {
    const value = check();
    if (value) return value;
    await sleep(20);
  }
  throw new Error(`timeout: ${label}`);
}

function start(t, extra = {}) {
  const child = spawn(binary, [], {
    env: {
      PATH: process.env.PATH,
      YOINK_RUST_HTTP_BIND: '127.0.0.1:0',
      YOINK_RUST_TELEGRAM_ENABLED: 'false',
      YOINK_RUST_SHUTDOWN_TIMEOUT_SECS: '2',
      YOINK_RUST_STARTUP_TIMEOUT_SECS: '2',
      ...extra,
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let output = '';
  let exited;
  let spawnError;
  child.on('error', error => { spawnError = error; });
  child.on('exit', (code, signal) => { exited = { code, signal }; });
  for (const stream of [child.stdout, child.stderr]) {
    stream.setEncoding('utf8');
    stream.on('data', chunk => { output = (output + chunk).slice(-16384); });
  }
  t.after(async () => {
    if (!exited && !spawnError) {
      child.kill('SIGKILL');
      await until(() => exited, 'fixture cleanup');
    }
  });
  return {
    child,
    output: () => output,
    async address(ready = true) {
      return until(() => {
        if (spawnError) throw spawnError;
        if (exited) throw new Error(`server exited before readiness: ${output}`);
        return output.includes(ready ? 'diagnostic adapters ready' : 'HTTP diagnostics listening') && output.match(/127\.0\.0\.1:\d+/)?.[0];
      }, 'server readiness');
    },
    async exit() {
      return until(() => {
        if (spawnError) throw spawnError;
        return exited;
      }, 'server exit');
    },
  };
}

async function listen(t, server) {
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  t.after(async () => {
    server.closeAllConnections?.();
    await new Promise(resolve => server.close(resolve));
  });
  return `127.0.0.1:${server.address().port}`;
}

const telegramConfig = address => ({
  YOINK_RUST_TELEGRAM_ENABLED: 'true',
  YOINK_RUST_TELEGRAM_TOKEN: fakeToken,
  YOINK_RUST_OWNER_ID: '42',
  YOINK_RUST_BOT_API_URL: `http://${address}`,
});

test('HTTP diagnostics, planned catalog and clean SIGTERM', async t => {
  const process = start(t);
  const base = `http://${await process.address()}`;
  for (const endpoint of ['/health/live', '/health/ready']) {
    assert.equal((await fetch(base + endpoint)).status, 200);
  }
  const snapshot = await (await fetch(base + '/api/v1/system')).json();
  assert.equal(snapshot.ready, true);
  assert.deepEqual(snapshot.modules.map(m => m.id), ['downloads', 'inbox', 'insight', 'music', 'stats']);
  assert(snapshot.modules.every(m => m.status === 'planned'));
  for (const endpoint of ['/api/v1/auth/token', '/api/v1/auth/dev']) {
    assert.equal((await fetch(base + endpoint, { method: 'POST' })).status, 404);
  }
  for (const endpoint of ['/api/v1/users/me', '/api/v1/settings']) {
    assert.equal((await fetch(base + endpoint)).status, 404);
  }
  assert.equal((await fetch(base + '/missing')).status, 404);
  assert.equal((await fetch(base + '/health/live', { method: 'POST' })).status, 405);
  process.child.kill('SIGTERM');
  assert.deepEqual(await process.exit(), { code: 0, signal: null });
});

test('native WebApp authentication, restart persistence and failed-startup isolation', async t => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'yoink-auth-smoke-'));
  await chmod(directory, 0o700);
  const secret = 'fixture-jwt-secret-at-least-32bytes';
  const env = {
    PATH: process.env.PATH,
    YOINK_RUST_DATABASE_PATH: path.join(directory, 'auth.db'),
    YOINK_RUST_TELEGRAM_ENABLED: 'false',
    YOINK_RUST_AUTH_ENABLED: 'true',
    YOINK_RUST_TELEGRAM_TOKEN: fakeToken,
    YOINK_RUST_API_SECRET_KEY: secret,
    YOINK_RUST_OWNER_ID: '42',
  };
  function signed(id) {
    const date = String(Math.floor(Date.now() / 1000));
    const user = JSON.stringify({ id, first_name: 'Native + %2F', username: 'fixture_user' });
    const key = createHmac('sha256', 'WebAppData').update(fakeToken).digest();
    const hash = createHmac('sha256', key).update(`auth_date=${date}\nuser=${user}`).digest('hex');
    return new URLSearchParams({ auth_date: date, user, hash }).toString();
  }
  const postLogin = (base, data) => fetch(base + '/api/v1/auth/token', {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ init_data: data }),
  });
  let server;
  try {
    await promisify(execFile)(binary, ['migrate'], { env, timeout: 15000 });
    server = start(t, env);
    let base = `http://${await server.address()}`;
    assert.equal((await postLogin(base, 'hash=invalid')).status, 401);
    const response = await postLogin(base, signed(42));
    assert.equal(response.status, 200);
    assert.equal(response.headers.get('cache-control'), 'no-store');
    const login = await response.json();
    assert.equal(login.role, 'owner');
    assert.equal(login.token_type, 'bearer');
    const headers = { authorization: `Bearer ${login.access_token}`, 'content-type': 'application/json' };
    let profile = await (await fetch(base + '/api/v1/users/me', { headers })).json();
    assert.equal(profile.id, 42);
    assert.equal(profile.first_name, 'Native + %2F');
    assert.equal((await fetch(base + '/api/v1/settings', { method: 'PATCH', headers, body: JSON.stringify({ language: 'ru', theme: 'mocha' }) })).status, 200);
    assert.equal((await fetch(base + '/api/v1/auth/dev?user_id=42', { method: 'POST' })).status, 404);
    server.child.kill('SIGTERM');
    assert.equal((await server.exit()).code, 0);
    assert(!server.output().includes(fakeToken));
    assert(!server.output().includes(secret));
    assert(!server.output().includes(login.access_token));

    let preparing = false;
    const api = http.createServer(() => { preparing = true; });
    const apiAddress = await listen(t, api);
    server = start(t, { ...env, ...telegramConfig(apiAddress), YOINK_RUST_STARTUP_TIMEOUT_SECS: '1' });
    base = `http://${await server.address(false)}`;
    await until(() => preparing, 'Telegram preparation entered');
    assert.equal((await fetch(base + '/health/ready')).status, 503);
    assert.equal((await postLogin(base, signed(43))).status, 503);
    assert.equal((await fetch(base + '/api/v1/settings', { method: 'PATCH', headers, body: JSON.stringify({ language: 'en' }) })).status, 503);
    assert.equal((await server.exit()).code, 1);
    assert.match(server.output(), /initialization exceeded the configured deadline/);
    assert(!server.output().includes(fakeToken));
    assert(!server.output().includes(secret));

    server = start(t, { ...env, YOINK_RUST_DEV_AUTH_ENABLED: 'true' });
    base = `http://${await server.address()}`;
    assert.deepEqual(await (await fetch(base + '/api/v1/settings', { headers })).json(), { user_id: 42, language: 'ru', theme: 'mocha' });
    profile = await (await fetch(base + '/api/v1/users/me', { headers })).json();
    assert.equal(profile.role, 'owner');
    assert.equal((await fetch(base + '/api/v1/auth/dev?user_id=43&role=owner', { method: 'POST' })).status, 401);
    assert.equal((await fetch(base + '/api/v1/auth/dev?user_id=42', { method: 'POST' })).status, 200);

    const ordinary = await (await postLogin(base, signed(43))).json();
    const ordinaryHeaders = { authorization: `Bearer ${ordinary.access_token}`, 'content-type': 'application/json' };
    assert.equal((await fetch(base + '/api/v1/settings', { method: 'PATCH', headers: ordinaryHeaders, body: JSON.stringify({ language: 'ru' }) })).status, 200);
    server.child.kill('SIGTERM');
    assert.equal((await server.exit()).code, 0);
    let polls = 0;
    const sent = [];
    const menus = new CommandMenus();
    const message = (id, actor, text) => ({
      message_id: id, date: 1700000000,
      from: { id: actor, is_bot: actor === fakeMe.id, first_name: 'Fixture', language_code: 'en' },
      chat: { id: 43, type: 'private', first_name: 'Fixture' }, text,
    });
    const botApi = http.createServer(async (request, response) => {
      response.setHeader('content-type', 'application/json');
      const method = request.url.split('/').at(-1).toLowerCase();
      let result;
      if (method === 'getme') result = fakeMe;
      else if (method === 'getupdates') {
        result = ++polls === 1 ? [{ update_id: 1, message: message(1, 43, '/help') }] : [];
        await sleep(30);
      } else if (['setmycommands', 'deletemycommands', 'getmycommands'].includes(method)) {
        let body = '';
        for await (const chunk of request) body += chunk;
        const data = JSON.parse(body);
        if (method === 'setmycommands') menus.set(data);
        if (method === 'deletemycommands') menus.delete(data);
        result = method === 'getmycommands' ? menus.get(data) : true;
      } else if (method === 'sendmessage') {
        let body = '';
        for await (const chunk of request) body += chunk;
        sent.push(JSON.parse(body));
        result = message(2, fakeMe.id, sent.at(-1).text);
      } else {
        response.writeHead(400);
        response.end(JSON.stringify({ ok: false, error_code: 400, description: 'unexpected method' }));
        return;
      }
      response.end(JSON.stringify({ ok: true, result }));
    });
    const botAddress = await listen(t, botApi);
    server = start(t, { ...env, ...telegramConfig(botAddress), YOINK_RUST_DOWNLOADS_ENABLED: 'false' });
    await server.address();
    await until(() => sent.length > 0 && polls >= 2, 'media-disabled localized ordinary help').catch(error => {
      throw new Error(`${error.message}; polls=${polls}; sends=${sent.length}; ${server.output()}`);
    });
    server.child.kill('SIGTERM');
    assert.equal((await server.exit()).code, 0);
    assert.equal(sent.length, 1);
    assert.deepEqual(menus.privateCommands(43, 'en').map(({ command }) => command).sort(), ['help', 'start'], 'media-disabled ordinary menu must omit unavailable handlers');
    assert.equal(Number(sent[0].chat_id), 43);
    assert.match(sent[0].text, /видео|загруз|аудио/i);
    assert.doesNotMatch(sent[0].text, /\/cookie|\/proxy|\/status/);
    assert(!server.output().includes(fakeToken));
    assert(!server.output().includes(ordinary.access_token));
  } finally {
    if (server) {
      server.child.kill('SIGTERM');
      await server.exit();
    }
    await rm(directory, { recursive: true, force: true });
  }
});

test('invalid configuration exits without printing supplied secrets', async t => {
  const process = start(t, {
    ...telegramConfig('127.0.0.1:1'),
    YOINK_RUST_BOT_API_URL: `http://user:${fakeToken}@127.0.0.1:1/`,
  });
  assert.equal((await process.exit()).code, 1);
  assert.match(process.output(), /invalid configuration YOINK_RUST_BOT_API_URL/);
  assert(!process.output().includes(fakeToken));
  assert(!process.output().includes('diagnostic adapters ready'));
});

test('occupied port fails startup', async t => {
  const address = await listen(t, net.createServer());
  const process = start(t, { YOINK_RUST_HTTP_BIND: address });
  assert.equal((await process.exit()).code, 1);
  assert.match(process.output(), /could not bind HTTP listener/);
});

test('Telegram startup rejection is sanitized and never ready', async t => {
  const server = http.createServer((request, response) => {
    response.writeHead(401, { 'content-type': 'application/json' });
    response.end(JSON.stringify({ ok: false, error_code: 401, description: `bad token ${fakeToken}` }));
  });
  const address = await listen(t, server);
  const process = start(t, telegramConfig(address));
  assert.equal((await process.exit()).code, 1);
  assert.match(process.output(), /Telegram initialization failed/);
  assert(!process.output().includes(fakeToken));
  assert(!process.output().includes('diagnostic adapters ready'));
});

test('readiness is 503 during slow startup and SIGTERM cancels initialization', async t => {
  const server = http.createServer(() => {});
  const address = await listen(t, server);
  const process = start(t, telegramConfig(address));
  const base = `http://${await process.address(false)}`;
  assert.equal((await fetch(base + '/health/live')).status, 200);
  assert.equal((await fetch(base + '/health/ready')).status, 503);
  assert.equal((await (await fetch(base + '/api/v1/system')).json()).ready, false);
  process.child.kill('SIGTERM');
  assert.deepEqual(await process.exit(), { code: 0, signal: null });
  assert(!process.output().includes('diagnostic adapters ready'));
});

test('startup deadline closes diagnostics and fails without exposing credentials', async t => {
  const address = await listen(t, http.createServer(() => {}));
  const process = start(t, { ...telegramConfig(address), YOINK_RUST_STARTUP_TIMEOUT_SECS: '1' });
  await process.address(false);
  assert.equal((await process.exit()).code, 1);
  assert.match(process.output(), /initialization exceeded the configured deadline/);
  assert(!process.output().includes(fakeToken));
});

test('polling failure stops the process rather than leaving false readiness', async t => {
  const server = http.createServer((request, response) => {
    response.setHeader('content-type', 'application/json');
    if (request.url.toLowerCase().endsWith('/getme')) {
      response.end(JSON.stringify({ ok: true, result: fakeMe }));
    } else if (/\/(setmycommands|deletemycommands)$/i.test(request.url)) {
      request.resume();
      response.end(JSON.stringify({ ok: true, result: true }));
    } else {
      response.writeHead(409);
      response.end(JSON.stringify({ ok: false, error_code: 409, description: `conflict ${fakeToken}` }));
    }
  });
  const address = await listen(t, server);
  const process = start(t, telegramConfig(address));
  assert.equal((await process.exit()).code, 1);
  assert.match(process.output(), /Telegram adapter failed/);
  assert(!process.output().includes(fakeToken));
});

test('Telegram polling remains driven during graceful shutdown', async t => {
  let polls = 0;
  const pollTimeouts = [];
  let webhookCalls = 0;
  const server = http.createServer((request, response) => {
    const method = request.url.split('/').at(-1).toLowerCase();
    response.setHeader('content-type', 'application/json');
    if (method === 'getme') {
      response.end(JSON.stringify({ ok: true, result: fakeMe }));
    } else if (method === 'getupdates') {
      polls += 1;
      let body = '';
      request.setEncoding('utf8');
      request.on('data', chunk => { body += chunk; });
      request.on('end', () => { pollTimeouts.push(JSON.parse(body || '{}').timeout); });
      // Deliberately hold an in-flight poll across SIGTERM. Its completion must
      // not require dropping the dispatcher future or the shutdown deadline.
      setTimeout(() => response.end(JSON.stringify({ ok: true, result: [] })), 100);
    } else if (['setmycommands', 'deletemycommands'].includes(method)) {
      request.resume();
      response.end(JSON.stringify({ ok: true, result: true }));
    } else if (method === 'deletewebhook') {
      webhookCalls += 1;
      response.end(JSON.stringify({ ok: true, result: true }));
    } else {
      response.writeHead(400);
      response.end(JSON.stringify({ ok: false, error_code: 400, description: 'unexpected method' }));
    }
  });
  const address = await listen(t, server);
  const process = start(t, telegramConfig(address));
  await process.address();
  await until(() => polls > 0, 'first getUpdates');
  process.child.kill('SIGTERM');
  assert.deepEqual(await process.exit(), { code: 0, signal: null });
  assert(pollTimeouts.includes(30), 'normal polls must use the configured long-poll timeout');
  assert.equal(webhookCalls, 0);
  assert(!process.output().includes(fakeToken));
});

test('actual dispatcher answers only the owner private chat in its language', async t => {
  let polls = 0;
  const sent = [];
  const menus = new CommandMenus();
  const message = (id, actor, chat, text = '/status', language = 'ru') => ({
    message_id: id, date: 1700000000,
    from: { id: actor, is_bot: false, first_name: 'Fixture', language_code: language },
    chat: chat > 0 ? { id: chat, type: 'private', first_name: 'Fixture' } : { id: chat, type: 'group', title: 'Fixture group' },
    text,
  });
  const server = http.createServer(async (request, response) => {
    response.setHeader('content-type', 'application/json');
    const method = request.url.split('/').at(-1).toLowerCase();
    let result;
    if (method === 'getme') result = fakeMe;
    else if (method === 'getupdates') {
      polls += 1;
      result = polls === 1 ? [
        { update_id: 1, message: message(1, 42, 42, '/status@yoink_fixture_bot') },
        { update_id: 2, message: message(2, 43, 43) },
        { update_id: 3, message: message(3, 42, -100) },
        { update_id: 4, message: message(4, 42, 42, '/status@another_bot') },
      ] : [];
      await sleep(30);
    } else if (['setmycommands', 'deletemycommands', 'getmycommands'].includes(method)) {
      let body = '';
      for await (const chunk of request) body += chunk;
      const data = JSON.parse(body);
      if (method === 'setmycommands') menus.set(data);
      if (method === 'deletemycommands') menus.delete(data);
      result = method === 'getmycommands' ? menus.get(data) : true;
    } else if (method === 'sendmessage') {
      let body = '';
      for await (const chunk of request) body += chunk;
      sent.push(JSON.parse(body));
      result = message(5, fakeMe.id, 42, sent.at(-1).text);
    } else {
      response.writeHead(400);
      response.end(JSON.stringify({ ok: false, error_code: 400, description: 'unexpected method' }));
      return;
    }
    response.end(JSON.stringify({ ok: true, result }));
  });
  const address = await listen(t, server);
  const process = start(t, telegramConfig(address));
  await process.address();
  await until(() => sent.length >= 1 && polls >= 2, 'diagnostic reply');
  process.child.kill('SIGTERM');
  assert.deepEqual(await process.exit(), { code: 0, signal: null });
  assert.equal(sent.length, 1);
  assert.deepEqual(menus.privateCommands(42, 'en').map(({ command }) => command).sort(), ['help', 'start', 'status'], 'media-disabled owner menu retains diagnostics only');
  assert(!menus.privateCommands(43, 'en').some(({ command }) => command === 'status'), 'owner controls cannot leak through global fallback');
  assert.equal(sent[0].chat_id, 42);
  assert.match(sent[0].text, /запланированы и пока недоступны/);
  assert(!process.output().includes(fakeToken));
});
