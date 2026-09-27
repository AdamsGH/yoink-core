// End-to-end media acceptance against an isolated PostgreSQL fixture and the built backend image.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { chmod, mkdir, mkdtemp, readFile, rm } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { setTimeout as sleep } from 'node:timers/promises';

const image = 'yoink/rust-backend:experimental';
const pgImage = 'postgres:16-alpine';
const workspace = fileURLToPath(new URL('../', import.meta.url));
const migrations = fileURLToPath(new URL('../../src/db', import.meta.url));
const legacyFixtures = fileURLToPath(new URL('../crates/yoink-identity/tests/fixtures.sql', import.meta.url));
const runId = `${process.pid}-${randomUUID().slice(0, 8)}`;
const pgName = `yoink-media-pg-${runId}`;
const appName = `yoink-media-app-${runId}`;
const password = randomUUID();
const token = '123456:fixture-only-token';
const secretMarker = 'source-secret-marker-' + randomUUID();
const tempRoot = await mkdtemp(path.join(os.tmpdir(), 'yoink-media-'));
const fixtureDir = path.join(tempRoot, 'fixture');
const containers = new Set();
const children = new Set();
const maxLog = 24 * 1024;
let interrupted = false;
let updateId = 1;
let nextMessageId = 1000;
let updateQueue = [];
const telegramCalls = [];
let mediaRequests = 0;
let mediaBytes;
let server;
let mediaServer;
let fixtureError;
const slowResponses = new Set();
const gatedResponses = new Set();
let mediaGateOpen = false;
let revokedQueuedFetches = 0;
const mediaRequestTimes = [];
let mediaSendMode = 'success';
let releaseUpload;
let mediaSendGate;
let mediaSendCalls = 0;

function bounded(value) {
  return String(value).slice(-maxLog);
}
function spawnCapture(command, args, { timeout = 30000, env = {}, input, ...options } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd: workspace, env: { PATH: process.env.PATH, ...env }, stdio: ['pipe', 'pipe', 'pipe'], ...options });
    children.add(child);
    let output = '';
    let settled = false;
    const append = chunk => { output = bounded(output + chunk.toString()); };
    child.stdout.on('data', append);
    child.stderr.on('data', append);
    if (input === undefined) child.stdin.end();
    else child.stdin.end(input);
    const timer = setTimeout(() => child.kill('SIGKILL'), timeout);
    child.once('error', error => finish(error));
    child.once('close', (code, signal) => finish(null, { code, signal, output }));
    function finish(error, result) {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      children.delete(child);
      if (error) reject(error);
      else if (result.code !== 0) reject(new Error(`${command} failed (exit ${result.code ?? result.signal}): ${result.output}`));
      else resolve(result);
    }
  });
}
async function docker(args, options = {}) {
  const { input, ...other } = options;
  const result = await spawnCapture('docker', args, { timeout: other.timeout ?? 30000, ...other, input });
  return result.output.trim();
}
async function startContainer(args) {
  const id = await docker(['run', '--detach', '--pull', 'never', ...args]);
  assert.match(id, /^[0-9a-f]{12,64}$/i, 'docker run must return a container ID');
  containers.add(id);
  return id;
}
let helperNumber = 0;
async function helperContainer(args, timeout = 30000) {
  const name = `yoink-media-helper-${runId}-${helperNumber++}`;
  const id = await docker(['create', '--pull', 'never', '--name', name, ...args]);
  assert.match(id, /^[0-9a-f]{12,64}$/i, 'docker create must return a container ID');
  containers.add(id);
  const result = await spawnCapture('docker', ['start', '--attach', id], { timeout });
  const exitCode = await docker(['inspect', '--format', '{{.State.ExitCode}}', id]);
  assert.equal(exitCode, '0', `fixture container ${name} failed: ${result.output}`);
  return result.output.trim();
}
async function cleanup() {
  for (const child of children) child.kill('SIGKILL');
  const failures = [];
  for (const id of containers) {
    try { await docker(['rm', '--force', '--volumes', id], { timeout: 15000 }); }
    catch (error) { failures.push(error); }
  }
  containers.clear();
  for (const item of [server, mediaServer]) {
    if (!item) continue;
    try {
      item.closeAllConnections?.();
      await new Promise((resolve, reject) => item.close(error => error ? reject(error) : resolve()));
    } catch (error) { failures.push(error); }
  }
  try { await rm(tempRoot, { recursive: true, force: true }); }
  catch (error) { failures.push(error); }
  if (failures.length) throw new AggregateError(failures, 'one or more fixture cleanup actions failed');
}
for (const signal of ['SIGINT', 'SIGTERM']) {
  process.once(signal, () => {
    interrupted = true;
    process.exitCode = 1;
    for (const child of children) child.kill('SIGKILL');
  });
}
function telegramMessage(actor, chat, text, id = nextMessageId++, attachment, threadId) {
  const type = chat > 0 ? 'private' : 'group';
  return {
    message_id: id, date: 1700000000,
    from: { id: actor, is_bot: false, first_name: 'Fixture', language_code: 'en' },
    chat: type === 'private' ? { id: chat, type } : { id: chat, type, title: 'Fixture group' },
    ...(text === null ? {} : { text }),
    ...(threadId === undefined ? {} : { message_thread_id: threadId }),
    ...(attachment ? { [attachment.kind]: attachment.value } : {}),
  };
}
function enqueue(actor, chat, text, id) {
  updateQueue.push({ update_id: updateId++, message: telegramMessage(actor, chat, text, id) });
}
async function handleTelegram(req, res) {
  const method = (req.url.split('/').at(-1) ?? '').toLowerCase();
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  const body = Buffer.concat(chunks);
  res.setHeader('content-type', 'application/json');
  if (method === 'getme') {
    res.end(JSON.stringify({ ok: true, result: { id: 123456, is_bot: true, first_name: 'Fixture', username: 'yoink_fixture_bot', can_join_groups: false, can_read_all_group_messages: false, supports_inline_queries: false, has_main_web_app: false } }));
  } else if (method === 'deletewebhook') {
    res.end(JSON.stringify({ ok: true, result: true }));
  } else if (method === 'getwebhookinfo') {
    res.end(JSON.stringify({ ok: true, result: { url: '', has_custom_certificate: false, pending_update_count: 0, allowed_updates: ['message'] } }));
  } else if (method === 'getupdates') {
    const batch = updateQueue;
    updateQueue = [];
    res.end(JSON.stringify({ ok: true, result: batch }));
  } else if (method === 'sendmessage') {
    const data = JSON.parse(body.toString('utf8'));
    telegramCalls.push({ method, data });
    res.end(JSON.stringify({ ok: true, result: telegramMessage(123456, Number(data.chat_id), data.text) }));
  } else if (method === 'sendvideo' || method === 'senddocument') {
    mediaSendCalls++;
    const boundary = String(req.headers['content-type'] ?? '').match(/boundary=([^;]+)/)?.[1];
    assert.ok(boundary, `${method} must use multipart/form-data`);
    const delimiter = Buffer.from(`--${boundary}`);
    const parts = [];
    let offset = 0;
    while ((offset = body.indexOf(delimiter, offset)) >= 0) {
      const start = offset + delimiter.length;
      if (body.subarray(start, start + 2).toString() === '--') break;
      const headersEnd = body.indexOf(Buffer.from('\r\n\r\n'), start);
      assert(headersEnd >= 0, 'multipart part must have headers');
      let end = body.indexOf(delimiter, headersEnd + 4);
      assert(end >= 0, 'multipart part must terminate at a boundary');
      if (body.subarray(end - 2, end).toString() === '\r\n') end -= 2;
      parts.push({ headers: body.subarray(start, headersEnd).toString('latin1'), value: body.subarray(headersEnd + 4, end) });
      offset = end + 2;
    }
    const filePart = parts.find(part => /filename="media\.mp4"/.test(part.headers));
    assert.ok(filePart, `${method} must upload the validated bounded filename`);
    assert(filePart.value.length > 100 && filePart.value.length <= 2 * 1024 * 1024, 'uploaded media must stay within fixture bounds');
    const sourceMdat = isoBmffMdatPayloads(mediaBytes);
    const uploadedMdat = isoBmffMdatPayloads(filePart.value);
    assert(sourceMdat.length > 0 && uploadedMdat.length > 0, 'source and upload must contain mdat boxes');
    assert.deepEqual(uploadedMdat, sourceMdat, 'remuxed media must preserve source mdat bytes');
    const chatPart = parts.find(part => /name="chat_id"/.test(part.headers));
    const chat = Number(chatPart?.value.toString('latin1'));
    assert(Number.isSafeInteger(chat), 'multipart payload must carry chat_id');
    const threadPart = parts.find(part => /name="message_thread_id"/.test(part.headers));
    const threadId = threadPart ? Number(threadPart.value.toString('latin1')) : undefined;
    telegramCalls.push({ method, status: mediaSendMode, uploadLength: filePart.value.length, receivedAt: performance.now() });
    if (mediaSendMode === 'hold') {
      await mediaSendGate;
      res.destroy();
      return;
    }
    if (mediaSendMode === 'socket-drop') {
      res.destroy();
      return;
    }
    if (mediaSendMode === 'api-ambiguous') {
      res.statusCode = 500;
      res.end(JSON.stringify({ ok: false, error_code: 500, description: 'Internal Server Error' }));
      return;
    }
    if (mediaSendMode === 'rate-limit-once') {
      mediaSendMode = 'success';
      res.statusCode = 429;
      res.end(JSON.stringify({ ok: false, error_code: 429, description: 'Too Many Requests: retry after 2', parameters: { retry_after: 2 } }));
      return;
    }
    if (mediaSendMode === 'api-error') {
      res.statusCode = 400;
      res.end(JSON.stringify({ ok: false, error_code: 400, description: 'Bad Request: chat not found' }));
      return;
    }
    const kind = method === 'sendvideo' ? 'video' : 'document';
    const attachment = kind === 'video'
      ? { kind, value: { file_id: `fixture-video-${mediaSendCalls}`, file_unique_id: `fixture-video-unique-${mediaSendCalls}`, width: 160, height: 120, duration: 1, mime_type: 'video/mp4', file_size: filePart.value.length } }
      : { kind, value: { file_id: `fixture-document-${mediaSendCalls}`, file_unique_id: `fixture-document-unique-${mediaSendCalls}`, file_name: 'media.mp4', mime_type: 'video/mp4', file_size: filePart.value.length } };
    res.end(JSON.stringify({ ok: true, result: telegramMessage(123456, chat, null, 9000 + mediaSendCalls, attachment, threadId) }));
  } else {
    res.writeHead(404);
    res.end(JSON.stringify({ ok: false, error_code: 404, description: 'unexpected fixture API method' }));
  }
}
function isoBmffMdatPayloads(bytes) {
  assert(bytes.length <= 2 * 1024 * 1024, 'ISO-BMFF fixture exceeds parser bound');
  const payloads = [];
  let offset = 0;
  let firstBox = true;
  while (offset < bytes.length) {
    assert(bytes.length - offset >= 8, 'truncated ISO-BMFF box header');
    const size32 = bytes.readUInt32BE(offset);
    const type = bytes.toString('ascii', offset + 4, offset + 8);
    let headerSize = 8;
    let boxSize;
    if (size32 === 1) {
      assert(bytes.length - offset >= 16, 'truncated extended ISO-BMFF box header');
      const extended = bytes.readBigUInt64BE(offset + 8);
      assert(extended <= BigInt(Number.MAX_SAFE_INTEGER), 'extended ISO-BMFF box size exceeds safe integer range');
      boxSize = Number(extended);
      headerSize = 16;
    } else if (size32 === 0) {
      boxSize = bytes.length - offset;
    } else {
      boxSize = size32;
    }
    assert(boxSize >= headerSize && boxSize <= bytes.length - offset, 'invalid or truncated ISO-BMFF box size');
    if (firstBox) {
      assert.equal(type, 'ftyp', 'ISO-BMFF media must begin with ftyp');
      assert(boxSize >= headerSize + 8, 'truncated ftyp box payload');
      firstBox = false;
    }
    if (type === 'mdat') payloads.push(bytes.subarray(offset + headerSize, offset + boxSize));
    offset += boxSize;
  }
  assert.equal(offset, bytes.length, 'ISO-BMFF boxes must frame the complete media buffer');
  return payloads;
}
async function freeLoopbackPort() {
  const listener = net.createServer();
  await new Promise((resolve, reject) => listener.listen(0, '127.0.0.1', error => error ? reject(error) : resolve()));
  const port = listener.address().port;
  await new Promise((resolve, reject) => listener.close(error => error ? reject(error) : resolve()));
  return port;
}
function startTelegramApi() {
  server = http.createServer((req, res) => {
    void handleTelegram(req, res).catch(error => {
      fixtureError ??= error;
      res.destroy(error);
    });
  });
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => resolve(server.address().port));
  });
}
function startMediaServer() {
  mediaServer = http.createServer((req, res) => {
    mediaRequests++;
    mediaRequestTimes.push(Date.now());
    if (req.url.includes('case=revoked-queued')) revokedQueuedFetches++;
    if (req.url.startsWith('/gated')) {
      if (!mediaGateOpen) {
        gatedResponses.add(res);
        res.once('close', () => gatedResponses.delete(res));
        return;
      }
      res.writeHead(200, { 'content-type': 'video/mp4', 'content-length': mediaBytes.length });
      res.end(mediaBytes);
      return;
    }
    if (req.url.startsWith('/slow')) {
      slowResponses.add(res);
      res.on('close', () => slowResponses.delete(res));
      return;
    }
    if (req.url.startsWith('/transient')) {
      res.writeHead(503, { 'content-type': 'text/plain' });
      res.end('temporary source unavailable');
      return;
    }
    if (req.url.startsWith('/media')) {
      res.writeHead(200, { 'content-type': 'video/mp4', 'content-length': mediaBytes.length });
      res.end(mediaBytes);
    } else {
      res.writeHead(404);
      res.end();
    }
  });
  return new Promise((resolve, reject) => {
    mediaServer.once('error', reject);
    mediaServer.listen(0, '127.0.0.1', () => resolve(mediaServer.address().port));
  });
}
async function waitFor(predicate, label, milliseconds = 12000) {
  const deadline = Date.now() + milliseconds;
  while (Date.now() < deadline) {
    if (fixtureError) throw new Error(`fixture HTTP server assertion failed: ${fixtureError.message}`, { cause: fixtureError });
    if (interrupted) throw new Error('test interrupted');
    const value = await predicate();
    if (value) return value;
    await sleep(40);
  }
  throw new Error(`timeout waiting for ${label}`);
}
async function psql(sql) {
  return docker(['exec', pgName, 'psql', '-U', 'yoink_test', '-d', 'yoink_test', '-v', 'ON_ERROR_STOP=1', '-Atc', sql]);
}
async function queryJob(id) {
  assert.match(id, /^[0-9a-f-]{36}$/i, 'operation id must be a UUID');
  return (await psql(`SELECT status || '|' || COALESCE(delivered_message_id::text, '') || '|' || attempt::text FROM yoink_runtime.operations WHERE id = '${id}'`)).trim();
}
async function queryByMessage(messageId) {
  return (await psql(`SELECT id FROM yoink_runtime.operations WHERE source_message_id = ${Number(messageId)} ORDER BY created_at DESC LIMIT 1`)).trim();
}
async function assertWorkspaceClean() {
  assert.equal((await docker(['exec', appName, 'sh', '-c', 'test -z "$(ls -A /tmp/yoink-work)" && echo clean'])).trim(), 'clean', 'worker workspace must be empty');
}
function safeTelegramSummary() {
  return telegramCalls.map(({ method, data, status, uploadLength }) => ({ method, status, uploadLength, chat_id: data?.chat_id, text: data?.text }));
}
async function fixtureDiagnostics() {
  const status = await docker(['inspect', '--format', '{{.State.Status}} exit={{.State.ExitCode}}', appName]).catch(error => `unavailable: ${error.message}`);
  const logs = await docker(['logs', '--tail', '80', appName]).catch(error => `unavailable: ${error.message}`);
  const jobs = await psql("SELECT id, status, attempt, request->'target'->>'bot_id' AS bot_id, progress_stage, progress_percent FROM yoink_runtime.operations ORDER BY created_at DESC LIMIT 10").catch(error => `unavailable: ${error.message}`);
  return `app=${status}\njobs (safe fields):\n${redact(jobs)}\nlogs (bounded, redacted):\n${redact(logs)}\ntelegram method counts=${JSON.stringify(telegramCalls.reduce((counts, call) => (counts[call.method] = (counts[call.method] ?? 0) + 1, counts), {}))}\ntelegram safe calls=${JSON.stringify(safeTelegramSummary())}`;
}
function redact(value) {
  return bounded(String(value).split(token).join('[REDACTED_TOKEN]').split(password).join('[REDACTED_PASSWORD]').split(secretMarker).join('[REDACTED_SOURCE]'));
}
function assertNoSecret(output, where) {
  assert(!output.includes(secretMarker), `${where} leaked the source URL marker`);
  assert(!output.includes(token), `${where} leaked the Telegram fixture token`);
  assert(!output.includes(password), `${where} leaked the fixture database password`);
}
async function waitUntilReady() {
  for (let i = 0; i < 100; i++) {
    try {
      await docker(['exec', pgName, 'pg_isready', '-U', 'yoink_test', '-d', 'yoink_test']);
      return;
    } catch {
      await sleep(100);
    }
  }
  throw new Error('isolated PostgreSQL did not become ready');
}
async function seedLegacy() {
  await helperContainer(['--network', `container:${pgName}`, '--entrypoint', 'alembic',
    '--mount', `type=bind,source=${migrations},target=/fixture/db,readonly`,
    '--env', `database_url=postgresql+psycopg://yoink_test:${password}@127.0.0.1:5432/yoink_test`,
    'yoink/yoink:latest', '-c', '/fixture/db/alembic.ini', 'upgrade', 'head'], 60000);
  const fixtureSql = await readFile(legacyFixtures, 'utf8');
  const ownerSql = "INSERT INTO public.users (id, role, language, theme, is_premium, created_at, updated_at, ban_until) VALUES (42, 'owner', 'en', 'macchiato', false, now(), now(), NULL), (43, 'user', 'en', 'macchiato', false, now(), now(), NULL);";
  await docker(['exec', '-i', pgName, 'psql', '-U', 'yoink_test', '-d', 'yoink_test', '-v', 'ON_ERROR_STOP=1'], { input: `${fixtureSql}\n${ownerSql}\n` });
}
async function runtimeMigrate() {
  await helperContainer(['--network', 'host', '--entrypoint', 'yoink-server',
    '--env', `YOINK_RUST_DATABASE_URL=postgresql://yoink_test:${password}@127.0.0.1:${pgPort}/yoink_test`,
    image, 'migrate'], 30000);
}
let pgPort;
let appContainerId;
let appHttpPort;
let apiPort;
let sourcePort;
let databaseUrl;
async function launchApp() {
  appContainerId = await startContainer(['--name', appName, '--label', 'yoink.fixture=media-app', '--network', 'host',
    '--read-only', '--tmpfs', '/tmp/yoink-work:rw,noexec,nosuid,size=256m,uid=10001,gid=10001,mode=0700',
    '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', '--pids-limit', '128', '--memory', '512m',
    '--env', 'YOINK_RUST_TELEGRAM_ENABLED=true', '--env', `YOINK_RUST_TELEGRAM_TOKEN=${token}`,
    '--env', 'YOINK_RUST_OWNER_ID=42', '--env', `YOINK_RUST_BOT_API_URL=http://127.0.0.1:${apiPort}`,
    '--env', `YOINK_RUST_DATABASE_URL=${databaseUrl}`, '--env', 'YOINK_RUST_DOWNLOADS_ENABLED=true',
    '--env', `YOINK_RUST_HTTP_BIND=127.0.0.1:${appHttpPort}`,
    '--env', `YOINK_RUST_WORKER_ALLOWED_PRIVATE_ORIGINS=["http://127.0.0.1:${sourcePort}"]`,
    '--env', 'YOINK_RUST_DOWNLOAD_LEASE_SECS=6', '--env', 'YOINK_RUST_DOWNLOAD_HEARTBEAT_SECS=1',
    '--env', 'YOINK_RUST_DELIVERY_TIMEOUT_SECS=2', '--env', 'YOINK_RUST_SHUTDOWN_TIMEOUT_SECS=10',
    '--env', 'YOINK_RUST_DOWNLOAD_TIMEOUT_SECS=3', '--env', 'YOINK_RUST_WORKER_SOCKET_TIMEOUT_SECS=1',
    '--env', 'YOINK_RUST_DOWNLOAD_RETRY_DELAY_SECS=1', '--env', 'YOINK_RUST_DOWNLOAD_POLL_INTERVAL_MS=50',
    '--env', 'YOINK_RUST_DOWNLOAD_CONCURRENCY=1', '--env', 'YOINK_RUST_DOWNLOAD_MAX_PENDING=3',
    '--env', 'YOINK_RUST_DOWNLOAD_MAX_PENDING_PER_USER=2', image]);
}
async function stopApp() {
  if (!appContainerId) return;
  assertNoSecret(await docker(['logs', appContainerId]), 'application logs before restart');
  await docker(['rm', '--force', appContainerId]);
  containers.delete(appContainerId);
  appContainerId = undefined;
}
async function restartApp() {
  await stopApp();
  await launchApp();
  await sleep(350);
}
async function operationForReply(fromIndex) {
  return await waitFor(() => {
    const reply = telegramCalls.slice(fromIndex).find(call => call.method === 'sendmessage' && /[0-9a-f]{8}-[0-9a-f-]{27}/i.test(call.data.text));
    return reply?.data.text.match(/[0-9a-f]{8}-[0-9a-f-]{27}/i)?.[0];
  }, 'operation admission reply');
}

try {
  await mkdir(fixtureDir);
  await chmod(fixtureDir, 0o777);
  await startContainer(['--name', pgName, '--label', 'yoink.fixture=media-pg', '--publish', '127.0.0.1::5432',
    '--tmpfs', '/var/lib/postgresql/data:rw', '--env', 'POSTGRES_USER=yoink_test', '--env', 'POSTGRES_DB=yoink_test',
    '--env', `POSTGRES_PASSWORD=${password}`, pgImage]);
  const binding = await docker(['port', pgName, '5432/tcp']);
  pgPort = binding.match(/^127\.0\.0\.1:(\d+)/m)?.[1];
  assert.ok(pgPort, 'disposable PostgreSQL must bind only to loopback');
  await waitUntilReady();
  await seedLegacy();
  await runtimeMigrate();

  // The built application image creates the actual media fixture; only this one-off
  // ffmpeg process receives the narrowly scoped writable fixture directory.
  await helperContainer(['--network', 'none', '--entrypoint', 'ffmpeg',
    '--mount', `type=bind,source=${fixtureDir},target=/fixture`, image,
    '-hide_banner', '-loglevel', 'error', '-f', 'lavfi', '-i', 'color=c=blue:s=160x120:d=1',
    '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=44100:duration=1', '-map', '0:v:0', '-map', '1:a:0',
    '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-movflags', '+faststart', '/fixture/sample.mp4'], 20000);
  mediaBytes = await readFile(path.join(fixtureDir, 'sample.mp4'));
  assert(mediaBytes.length > 100, 'ffmpeg must produce nonempty media');
  assert(isoBmffMdatPayloads(mediaBytes).length > 0, 'ffmpeg fixture must contain framed MP4 media data');

  apiPort = await startTelegramApi();
  sourcePort = await startMediaServer();
  appHttpPort = await freeLoopbackPort();
  databaseUrl = `postgresql://yoink_test:${password}@127.0.0.1:${pgPort}/yoink_test`;
  await launchApp();

  const sourceUrl = `http://127.0.0.1:${sourcePort}/media?key=${secretMarker}`;
  const replayMessageId = 500;
  enqueue(42, 42, `/download ${sourceUrl}`, replayMessageId);
  await waitFor(() => telegramCalls.some(call => call.method === 'sendmessage'), 'download admission reply');
  const admissionCall = telegramCalls.find(call => call.method === 'sendmessage');
  assert.equal(admissionCall.data.chat_id, 42);
  const operationId = admissionCall.data.text.match(/[0-9a-f]{8}-[0-9a-f-]{27}/i)?.[0];
  assert.ok(operationId, 'admission reply must contain its operation UUID');

  const succeeded = await waitFor(async () => {
    const row = await queryJob(operationId);
    return row.startsWith('succeeded|') ? row : false;
  }, 'successful media delivery');
  assert.match(succeeded, /^succeeded\|[1-9]\d*\|1$/);
  assert.equal(telegramCalls.filter(call => call.method === 'sendvideo' || call.method === 'senddocument').length, 1, 'successful work must send exactly one Telegram media attachment');
  assert(mediaRequests >= 1, 'worker must fetch the source fixture');
  const initialSourceRequests = mediaRequests;
  assert.equal((await docker(['exec', appName, 'sh', '-c', 'test -z "$(ls -A /tmp/yoink-work)" && echo clean'])).trim(), 'clean',
    'workspace must be empty after the acknowledged delivery');

  const originalMediaSendCount = telegramCalls.filter(call => call.method === 'sendvideo' || call.method === 'senddocument').length;
  enqueue(42, 42, `/download ${sourceUrl}`, replayMessageId);
  enqueue(42, 42, `/job ${operationId}`);
  enqueue(43, 43, `/job ${operationId}`);
  enqueue(42, -10042, `/download ${sourceUrl}`);
  enqueue(42, 42, `/cancel ${operationId}`);
  await waitFor(() => telegramCalls.filter(call => call.method === 'sendmessage').length >= 3, 'job and denial replies');
  await sleep(350);
  assert.equal(telegramCalls.filter(call => call.method === 'sendvideo' || call.method === 'senddocument').length, originalMediaSendCount,
    'replayed admission and status/cancel/group commands must not redeliver media');
  assert.equal(mediaRequests, initialSourceRequests, 'replayed update must not execute another download');
  const replies = telegramCalls.filter(call => call.method === 'sendmessage').map(call => call.data.text);
  assert(replies.some(text => text.includes(operationId) && text.includes('succeeded')), 'owner /job must return the succeeded operation status');
  assert.equal(telegramCalls.filter(call => call.method === 'sendmessage' && call.data.chat_id === 43).length, 0,
    'non-owner is ignored by the private owner gate');
  const unknownId = randomUUID();
  enqueue(42, 42, `/job ${unknownId}`);
  await waitFor(() => telegramCalls.some(call => call.method === 'sendmessage' && call.data.text === 'Operation not found.'), 'unknown owner operation response');
  assertNoSecret(JSON.stringify(safeTelegramSummary()), 'Telegram replies');
  const logs = await docker(['logs', appName]);
  assertNoSecret(logs, 'application logs');

  const beforeCancelMediaSends = mediaSendCalls;
  const cancelMessage = 510;
  enqueue(42, 42, `/download http://127.0.0.1:${sourcePort}/slow?cancel=${cancelMessage}`, cancelMessage);
  const cancelId = await operationForReply(telegramCalls.length);
  await waitFor(() => mediaRequestTimes.length >= 2, 'active slow worker request');
  assert((await queryJob(cancelId)).startsWith('running|'), 'operation must be durably running before cancellation');
  const cancelReplyStart = telegramCalls.length;
  enqueue(42, 42, `/cancel ${cancelId}`);
  await waitFor(async () => (await queryJob(cancelId)).startsWith('cancelled|'), 'cancelled active operation');
  await assertWorkspaceClean();
  assert.equal(mediaSendCalls, beforeCancelMediaSends, 'cancelled download must not upload');
  for (const response of slowResponses) response.destroy();
  await waitFor(() => telegramCalls.slice(cancelReplyStart).some(call => call.method === 'sendmessage'), 'cancel response');

  const requestStart = mediaRequestTimes.length;
  const transientMessage = 520;
  enqueue(42, 42, `/download http://127.0.0.1:${sourcePort}/transient?case=${transientMessage}`, transientMessage);
  const transientId = await operationForReply(telegramCalls.length);
  const transientResult = await waitFor(async () => {
    const row = await queryJob(transientId);
    return row.startsWith('failed|') ? row : false;
  }, 'transient source failure', 20000);
  assert.match(transientResult, /^failed\|\|3$/);
  const transientTimes = mediaRequestTimes.slice(requestStart);
  assert(transientTimes.length >= 3, '503 must be retried to persisted attempt three');
  assert(transientTimes.at(-1) - transientTimes[0] >= 1000, 'retry interval must be observable at the source');
  assert.equal(mediaSendCalls, beforeCancelMediaSends, 'transient source failures must not upload');

  mediaSendMode = 'api-error';
  const apiErrorMediaSends = mediaSendCalls;
  const apiErrorMessage = 530;
  enqueue(42, 42, `/download ${sourceUrl}&case=${apiErrorMessage}`, apiErrorMessage);
  const apiErrorId = await operationForReply(telegramCalls.length);
  const apiErrorResult = await waitFor(async () => {
    const row = await queryJob(apiErrorId);
    return row.startsWith('failed|') ? row : false;
  }, 'explicit Telegram API failure');
  assert.match(apiErrorResult, /^failed\|\|1$/);
  assert.equal(mediaSendCalls - apiErrorMediaSends, 1, 'explicit Telegram API failure must be attempted exactly once');

  mediaSendMode = 'rate-limit-once';
  const rateLimitedStart = telegramCalls.length;
  const rateLimitedSends = mediaSendCalls;
  enqueue(42, 42, `/download ${sourceUrl}&case=rate-limited`, 532);
  const rateLimitedId = await operationForReply(rateLimitedStart);
  const rateLimitedResult = await waitFor(async () => {
    const row = await queryJob(rateLimitedId);
    return row.startsWith('succeeded|') ? row : false;
  }, 'durable retry after a definitively unsent Telegram rate limit', 20000);
  assert.match(rateLimitedResult, /^succeeded\|\d+\|2$/);
  assert.equal(mediaSendCalls - rateLimitedSends, 2, 'rate-limited unit must be retried once through a new durable attempt');
  const rateLimitedCalls = telegramCalls.slice(rateLimitedStart).filter(call => call.method === 'sendvideo' || call.method === 'senddocument');
  assert.equal(rateLimitedCalls[0].status, 'rate-limit-once');
  assert.equal(rateLimitedCalls[1].status, 'success');
  assert(rateLimitedCalls[1].receivedAt - rateLimitedCalls[0].receivedAt >= 2000, 'Telegram retry_after must not be shortened to the configured one-second retry delay');

  mediaSendMode = 'api-ambiguous';
  enqueue(42, 42, `/download ${sourceUrl}&case=api-500`, 535);
  const apiUnknownId = await operationForReply(telegramCalls.length);
  await waitFor(async () => (await queryJob(apiUnknownId)).startsWith('delivery_unknown|'), 'API 500 delivery uncertainty');
  assert.equal(telegramCalls.filter(call => (call.method === 'sendvideo' || call.method === 'senddocument') && call.status === 'api-ambiguous').length, 1);
  mediaSendMode = 'socket-drop';

  const unknownMediaSends = mediaSendCalls;
  const unknownMessage = 540;
  enqueue(42, 42, `/download ${sourceUrl}&case=${unknownMessage}`, unknownMessage);
  const deliveryUnknownId = await operationForReply(telegramCalls.length);
  await waitFor(async () => (await queryJob(deliveryUnknownId)).startsWith('delivery_unknown|'), 'ambiguous delivery status');
  const warningStart = telegramCalls.length;
  enqueue(42, 42, `/job ${deliveryUnknownId}`);
  await waitFor(() => telegramCalls.slice(warningStart).some(call => call.method === 'sendmessage' && call.data.text.includes(deliveryUnknownId) && /could not be confirmed/i.test(call.data.text) && /check the chat before submitting again/i.test(call.data.text)), 'job delivery uncertainty warning');
  await restartApp();
  await sleep(6500);
  assert((await queryJob(deliveryUnknownId)).startsWith('delivery_unknown|'), 'restart must preserve ambiguous delivery state');
  assert((await queryJob(apiUnknownId)).startsWith('delivery_unknown|'), 'restart must preserve API 500 delivery uncertainty');
  assert.equal(telegramCalls.filter(call => (call.method === 'sendvideo' || call.method === 'senddocument') && call.status === 'api-ambiguous').length, 1);
  assert.equal(mediaSendCalls - unknownMediaSends, 1, 'ambiguous delivery must never be resent after restart');

  mediaSendMode = 'hold';
  mediaSendGate = new Promise(resolve => { releaseUpload = resolve; });
  const crashMediaSends = mediaSendCalls;
  const crashMessage = 550;
  enqueue(42, 42, `/download ${sourceUrl}&case=${crashMessage}`, crashMessage);
  const crashId = await operationForReply(telegramCalls.length);
  await waitFor(async () => (await queryJob(crashId)).startsWith('delivering|'), 'durable delivering state before crash');
  await waitFor(() => mediaSendCalls > crashMediaSends, 'blocked media send request');
  await stopApp();
  releaseUpload?.();
  releaseUpload = undefined;
  mediaSendMode = 'socket-drop';
  await launchApp();
  await waitFor(async () => (await queryJob(crashId)).startsWith('delivery_unknown|'), 'expired delivering lease recovery', 15000);
  assert.equal(mediaSendCalls - crashMediaSends, 1, 'expired in-flight delivery must not be sent again');

  const operationsBeforeBan = Number((await psql('SELECT count(*) FROM yoink_runtime.operations')).trim());
  await psql("UPDATE public.users SET ban_until = now() + interval '10 minutes' WHERE id = 42");
  const deniedStart = telegramCalls.length;
  enqueue(42, 42, `/download ${sourceUrl}&case=ban`, 560);
  await waitFor(() => telegramCalls.slice(deniedStart).some(call => call.method === 'sendmessage' && call.data.text === 'This operation is not allowed.'), 'banned actor denial');
  assert.equal(Number((await psql('SELECT count(*) FROM yoink_runtime.operations')).trim()), operationsBeforeBan, 'banned actor must not create a durable operation');
  await psql("UPDATE public.users SET ban_until = NULL WHERE id = 42");

  const mediaSendsBeforeQuota = mediaSendCalls;
  const slowA = 570;
  const quotaRequestBase = mediaRequests;
  enqueue(42, 42, `/download http://127.0.0.1:${sourcePort}/slow?quota=${slowA}`, slowA);
  const quotaA = await operationForReply(telegramCalls.length);
  await waitFor(() => mediaRequests > quotaRequestBase, 'first quota worker active');
  const slowB = 571;
  enqueue(42, 42, `/download http://127.0.0.1:${sourcePort}/slow?quota=${slowB}`, slowB);
  const quotaB = await operationForReply(telegramCalls.length);
  const rejectedStart = telegramCalls.length;
  const countBeforeRejection = Number((await psql('SELECT count(*) FROM yoink_runtime.operations')).trim());
  enqueue(42, 42, `/download ${sourceUrl}&case=quota-reject`, 572);
  await waitFor(() => telegramCalls.slice(rejectedStart).some(call => call.method === 'sendmessage' && call.data.text === 'The download queue is full. Try again later.'), 'per-user quota rejection');
  assert.equal(Number((await psql('SELECT count(*) FROM yoink_runtime.operations')).trim()), countBeforeRejection, 'quota rejection must not persist a third operation');
  assert.equal((await psql(`SELECT count(*) FROM yoink_runtime.operations WHERE id IN ('${quotaA}', '${quotaB}') AND status IN ('queued','running')`)).trim(), '2');
  enqueue(42, 42, `/cancel ${quotaA}`);
  enqueue(42, 42, `/cancel ${quotaB}`);
  await waitFor(async () => (await queryJob(quotaA)).startsWith('cancelled|') && (await queryJob(quotaB)).startsWith('cancelled|'), 'both quota operations cancelled');
  for (const response of slowResponses) response.destroy();
  await assertWorkspaceClean();
  assert.equal(mediaSendCalls, mediaSendsBeforeQuota, 'quota rejection and cancellation must not upload');

  mediaSendMode = 'success';
  const mediaSendsBeforeRevocation = mediaSendCalls;
  enqueue(42, 42, `/download http://127.0.0.1:${sourcePort}/gated?key=${secretMarker}`, 580);
  const runningRevoked = await operationForReply(telegramCalls.length);
  await waitFor(() => gatedResponses.size > 0, 'worker blocked on gated source');
  enqueue(42, 42, `/download ${sourceUrl}&case=revoked-queued`, 581);
  const queuedRevoked = await operationForReply(telegramCalls.length);
  assert((await queryJob(queuedRevoked)).startsWith('queued|'), 'second request must still be queued');
  await psql("UPDATE public.users SET ban_until = now() + interval '10 minutes' WHERE id = 42");
  mediaGateOpen = true;
  for (const response of gatedResponses) {
    response.writeHead(200, { 'content-type': 'video/mp4', 'content-length': mediaBytes.length });
    response.end(mediaBytes);
  }
  await waitFor(async () => (await queryJob(runningRevoked)).startsWith('failed|') && (await queryJob(queuedRevoked)).startsWith('failed|'), 'revoked access checked before delivery and before execution');
  assert.equal(revokedQueuedFetches, 0, 'revoked queued request must not execute its worker');
  assert.equal(mediaSendCalls, mediaSendsBeforeRevocation, 'revocation before delivery must prevent upload');
  await psql('UPDATE public.users SET ban_until = NULL WHERE id = 42');
  await assertWorkspaceClean();

  assertNoSecret(await docker(['logs', appContainerId]), 'application logs after failure scenarios');
  assertNoSecret(JSON.stringify(telegramCalls), 'Telegram output after failure scenarios');
  console.log('Media slice acceptance passed all required real-worker, durable-delivery, access, retry, cancellation, quota, restart and redaction scenarios.');
} catch (error) {
  console.error(`${redact(error.stack ?? error.message)}\\n${await fixtureDiagnostics().catch(diagnosticError => `diagnostics unavailable: ${diagnosticError.message}`)}`);
  process.exitCode = 1;
} finally {
  try {
    await cleanup();
  } catch (error) {
    console.error(`fixture cleanup failed: ${bounded(error.message)}`);
    process.exitCode = 1;
  }
  if (interrupted) process.exitCode = 1;
}
