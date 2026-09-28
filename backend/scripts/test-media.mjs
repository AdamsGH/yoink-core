// End-to-end media acceptance against an isolated native database fixture and the built backend image.
import assert from 'node:assert/strict';
import { createProcessRunner, createContainerLifecycle } from './media-fixture/process-lifecycle.mjs';
import { createNativeDbClient } from './media-fixture/native-db.mjs';
import { boundReport, writeFailureReport } from './media-fixture/failure-report.mjs';
import { isoBmffMdatPayloads } from './media-fixture/media-format.mjs';
import { randomBytes, randomUUID } from 'node:crypto';
import { BotApiFixture } from './media-fixture/bot-api.mjs';
import { runCommandMediaScenarios } from './media-fixture/command-scenarios.mjs';
import { runPreferenceScenarios } from './media-fixture/preference-scenarios.mjs';
import { runBatchScenarios } from './media-fixture/batch-scenarios.mjs';
import { runProgressScenarios } from './media-fixture/progress-scenarios.mjs';
import { runGroupScenarios } from './media-fixture/group-scenarios.mjs';
import { startGallerySource } from './media-fixture/gallery-source.mjs';
import { runGalleryScenarios } from './media-fixture/gallery-scenarios.mjs';
import { runSubtitleScenarios } from './media-fixture/subtitle-scenarios.mjs';
import { AuthenticatedProxyFixture, freeLoopbackPort, SourceHttpFixture } from './media-fixture/source-http.mjs';
import os from 'node:os';
import path from 'node:path';
import { chmod, mkdir, mkdtemp, readFile, rm } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { setTimeout as sleep } from 'node:timers/promises';

const scenario = process.argv[2] ?? 'all';
assert(['all', 'gallery', 'subtitles'].includes(scenario) && process.argv.length <= 3, 'usage: test-media.mjs [all|gallery|subtitles]');
const image = 'yoink/rust-backend:experimental';
const fixtureImage = 'yoink/rust-backend-fixture:experimental';
const workspace = fileURLToPath(new URL('../', import.meta.url));
const nativeFixtures = fileURLToPath(new URL('../crates/yoink-identity/tests/fixtures.sql', import.meta.url));
const runId = `${process.pid}-${randomUUID().slice(0, 8)}`;
const volumeName = `yoink-media-state-${runId}`;
const appName = `yoink-media-app-${runId}`;
const token = '123456:fixture-only-token';
const fixtureToken = randomBytes(32).toString('hex');
const secretMarker = 'source-secret-marker-' + randomUUID();
const cookieSecret = 'fixture-cookie-value-never-log';
const proxyUser = 'fixture-proxy-user';
const proxyPassword = 'fixture-proxy-password-never-log';
const proxyAuth = `Basic ${Buffer.from(`${proxyUser}:${proxyPassword}`).toString('base64')}`;
const tempRoot = await mkdtemp(path.join(os.tmpdir(), 'yoink-media-'));
const fixtureDir = path.join(tempRoot, 'fixture');
const containers = new Set();
const children = new Set();
const maxLog = 24 * 1024;
let interrupted = false;
let mediaBytes;
let botApi;
let sourceFixture;
let proxyFixture;
let appContainerId;
let appHttpPort;
let fixturePort;
let apiPort;
let sourcePort;
let proxyPort;
let volumeCreated = false;
let downloadMaxAttempts;
let downloadTimeoutSeconds = 3;
let workerSocketTimeoutSeconds = 1;
let cacheTtlSeconds;
let groupsEnabled = false;
let gallerySource;
let galleryEnabled = false;
let galleryTrusted = true;
const processRunner = createProcessRunner({ cwd: workspace, children, maxOutput: maxLog });
const { capture: spawnCapture, docker } = processRunner;
const containerLifecycle = createContainerLifecycle({ docker, capture: spawnCapture, containers, assert, runId });
const { startContainer, helperContainer } = containerLifecycle;
const nativeDb = createNativeDbClient({ getAppContainerId: () => appContainerId, getFixturePort: () => fixturePort, fixtureToken, volumeName, fixtureImage, helperContainer });
const { query: nativeQuery, batch: nativeBatch } = nativeDb;
async function cleanup() {
  for (const child of children) child.kill('SIGKILL');
  const failures = [];
  for (const id of containers) { try { await docker(['rm', '--force', '--volumes', id], { timeout: 15000 }); } catch (error) { failures.push(error); } }
  containers.clear();
  if (volumeCreated) { try { await docker(['volume', 'rm', volumeName], { timeout: 15000 }); } catch (error) { failures.push(error); } volumeCreated = false; }
  for (const fixture of [botApi, sourceFixture, proxyFixture, gallerySource]) { if (!fixture) continue; try { await fixture.close(); } catch (error) { failures.push(error); } }
  try { await rm(tempRoot, { recursive: true, force: true }); } catch (error) { failures.push(error); }
  if (failures.length) throw new AggregateError(failures, 'one or more fixture cleanup actions failed');
}
for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, () => { interrupted = true; process.exitCode = 1; for (const child of children) child.kill('SIGKILL'); });
async function waitFor(predicate, label, milliseconds = 12000) {
  const deadline = Date.now() + milliseconds;
  while (Date.now() < deadline) {
    const fixtureError = botApi?.error ?? sourceFixture?.error;
    if (fixtureError) throw new Error(`fixture HTTP server assertion failed: ${fixtureError.message}`, { cause: fixtureError });
    if (interrupted) throw new Error('test interrupted');
    const value = await predicate();
    if (value) return value;
    await sleep(40);
  }
  throw new Error(`timeout waiting for ${label}`);
}
async function fixtureSql(sql) {
  const statement = sql.trim();
  if (/^(SELECT|WITH)\b/i.test(statement) || /\bRETURNING\b/i.test(statement)) return nativeQuery(statement);
  await nativeBatch(statement);
  return '';
}
async function queryJob(id) {
  assert.match(id, /^[0-9a-f-]{36}$/i, 'operation id must be a UUID');
  return (await nativeQuery(`SELECT status || '|' || COALESCE(delivered_message_id, '') || '|' || attempt FROM runtime_operations WHERE id = '${id}'`)).trim();
}
async function queryByMessage(messageId) {
  return (await nativeQuery(`SELECT id FROM runtime_operations WHERE json_extract(request, '$.target.reply_to_message_id') = ${Number(messageId)} ORDER BY created_at DESC LIMIT 1`)).trim();
}
async function assertWorkspaceClean() {
  assert.equal((await docker(['exec', appName, 'sh', '-c', 'test -z "$(ls -A /tmp/yoink-work)" && echo clean'])).trim(), 'clean', 'worker workspace must be empty');
}
function redact(value) {
  return boundReport(String(value).split(token).join('[REDACTED_TOKEN]').split(fixtureToken).join('[REDACTED_FIXTURE_TOKEN]').split(secretMarker).join('[REDACTED_SOURCE]').split(cookieSecret).join('[REDACTED_COOKIE]').split(proxyPassword).join('[REDACTED_PROXY_PASSWORD]').split(proxyUser).join('[REDACTED_PROXY_USER]').split(proxyAuth).join('[REDACTED_PROXY_AUTH]'), maxLog);
}
function assertNoSecret(output, where) {
  for (const secret of [secretMarker, token, fixtureToken, cookieSecret, proxyPassword, proxyUser, proxyAuth]) assert(!output.includes(secret), `${where} leaked fixture credentials`);
  assert(!/fixture-(video|document|audio|photo)-\d+/.test(output), `${where} leaked a Telegram file ID`);
}
function safeTelegramSummary() { return botApi.calls.slice(-20).map(({method,data,status,uploadLength,cached}) => ({method,status,uploadLength,cached,chat_id:data?.chat_id,text:data?.text})); }
async function fixtureDiagnostics() {
  const status = await docker(['inspect','--format','{{.State.Status}} exit={{.State.ExitCode}}',appName]).catch(error => `unavailable: ${error.message}`);
  const logs = await docker(['logs','--tail','80',appName]).catch(error => `unavailable: ${error.message}`);
  const jobs = await nativeQuery('SELECT id,status,attempt,json_extract(request, \'$.target.bot_id\'),progress_stage,progress_percent FROM runtime_operations ORDER BY created_at DESC LIMIT 10').catch(error => `unavailable: ${error.message}`);
  return `app=${status}\njobs (safe fields):\n${redact(jobs)}\nlogs (bounded, redacted):\n${redact(logs)}\ntelegram calls (last 20 of ${botApi.calls.length}):\n${safeTelegramSummary().map(call => JSON.stringify(call)).join('\n')}`;
}
async function reportFailure(error, phase) {
  const details = error instanceof AggregateError
    ? error.errors.map(cause => cause.stack ?? String(cause)).join('\n')
    : error.stack ?? String(error);
  const diagnostics = phase === 'scenario'
    ? await fixtureDiagnostics().catch(cause => `diagnostics unavailable: ${cause.message}`)
    : '';
  const report = redact(`[${phase}]\n${details}\n${diagnostics}`);
  console.error(report);
  try {
    const destination = await writeFailureReport(path.join(workspace, 'target', 'media-reports'), `${runId}-${phase}`, report);
    console.error(`Media failure report: ${destination}`);
  } catch (cause) {
    console.error(`Could not retain media failure report: ${redact(cause.message)}`);
  }
}
async function launchApp() {
  appContainerId = await startContainer(['--name',appName,'--label','yoink.fixture=media-app','--network','host','--read-only',
    '--tmpfs','/tmp/yoink-work:rw,noexec,nosuid,size=256m,uid=10001,gid=10001,mode=0700',
    '--tmpfs','/tmp/yoink-db:rw,noexec,nosuid,nodev,size=64m,uid=10001,gid=10001,mode=0700',
    '--cap-drop','ALL','--security-opt','no-new-privileges','--pids-limit','128','--memory','512m',
    '--env','YOINK_RUST_TELEGRAM_PROGRESS_INTERVAL_SECS=1','--env','YOINK_RUST_TELEGRAM_PROGRESS_TIMEOUT_SECS=2',
    '--env','YOINK_RUST_TELEGRAM_ENABLED=true','--env',`YOINK_RUST_TELEGRAM_TOKEN=${token}`,'--env','YOINK_RUST_OWNER_ID=42',
    '--env',`YOINK_RUST_TELEGRAM_GROUPS_ENABLED=${groupsEnabled}`,
    '--env',`YOINK_RUST_BOT_API_URL=http://127.0.0.1:${apiPort}`,'--env','TURSO_TMPDIR=/tmp/yoink-db','--env','TMPDIR=/tmp/yoink-db',
    '--env','YOINK_RUST_DATABASE_PATH=/state/private/state.db',
    '--env',`YOINK_FIXTURE_BIND=127.0.0.1:${fixturePort}`,'--env',`YOINK_FIXTURE_TOKEN=${fixtureToken}`,
    '--mount',`type=volume,source=${volumeName},target=/state`,
    '--env','YOINK_RUST_DOWNLOADS_ENABLED=true','--env',`YOINK_RUST_HTTP_BIND=127.0.0.1:${appHttpPort}`,
    ...(galleryEnabled ? (galleryTrusted ? gallerySource.dockerArgs : gallerySource.untrustedDockerArgs) : []),
    '--env',`YOINK_RUST_WORKER_ALLOWED_PRIVATE_ORIGINS=${JSON.stringify([`http://127.0.0.1:${sourcePort}`, ...(galleryEnabled ? ['https://imgbox.com'] : [])])}`,
    '--env',`YOINK_RUST_WORKER_ALLOWED_PROXY_ORIGINS=["http://127.0.0.1:${proxyPort}"]`,
    '--env','YOINK_RUST_DOWNLOAD_LEASE_SECS=6','--env','YOINK_RUST_DOWNLOAD_HEARTBEAT_SECS=1','--env','YOINK_RUST_DELIVERY_TIMEOUT_SECS=2',
    '--env','YOINK_RUST_SHUTDOWN_TIMEOUT_SECS=10','--env',`YOINK_RUST_DOWNLOAD_TIMEOUT_SECS=${downloadTimeoutSeconds}`,'--env',`YOINK_RUST_WORKER_SOCKET_TIMEOUT_SECS=${workerSocketTimeoutSeconds}`,
    '--env','YOINK_RUST_DOWNLOAD_RETRY_DELAY_SECS=1','--env','YOINK_RUST_DOWNLOAD_POLL_INTERVAL_MS=50',
    '--env','YOINK_RUST_DOWNLOAD_CONCURRENCY=1','--env','YOINK_RUST_DOWNLOAD_MAX_PENDING=3','--env','YOINK_RUST_DOWNLOAD_MAX_PENDING_PER_USER=2',
    ...(downloadMaxAttempts===undefined?[]:['--env',`YOINK_RUST_DOWNLOAD_MAX_ATTEMPTS=${downloadMaxAttempts}`]),
    ...(cacheTtlSeconds===undefined?[]:['--env',`YOINK_RUST_DOWNLOAD_CACHE_TTL_SECS=${cacheTtlSeconds}`]),
    '--entrypoint','yoink-fixture-server',fixtureImage]);
}
async function stopApp() {
  if (!appContainerId) return;
  assertNoSecret(await docker(['logs',appContainerId]),'application logs before restart');
  await docker(['rm','--force',appContainerId]);
  containers.delete(appContainerId);
  appContainerId=undefined;
}
async function restartApp() { await stopApp(); await launchApp(); await sleep(350); }
async function operationForReply(fromIndex) {
  return await waitFor(() => {
    const reply=botApi.calls.slice(fromIndex).find(call=>call.method==='sendmessage'&&/[0-9a-f]{8}-[0-9a-f-]{27}/i.test(call.data.text));
    return reply?.data.text.match(/[0-9a-f]{8}-[0-9a-f-]{27}/i)?.[0];
  },'operation admission reply');
}
async function seedDatabase() {
  await docker(['volume','create',volumeName]);
  volumeCreated=true;
  await helperContainer(['--network','none','--user','0','--mount',`type=volume,source=${volumeName},target=/state`,'--entrypoint','sh',image,
    '-c','mkdir -p /state/private && chmod 0700 /state/private && chown 10001:10001 /state/private'],15000);
}
async function runtimeMigrate() {
  await helperContainer(['--network','none','--user','10001:10001','--mount',`type=volume,source=${volumeName},target=/state`,
    '--env','YOINK_RUST_DATABASE_PATH=/state/private/state.db',
    '--entrypoint','yoink-server',image,'migrate'],30000);
}
async function seedFixtureRows() {
  const fixtures=await readFile(nativeFixtures,'utf8');
  const owners=`INSERT INTO users(id,role,language,theme,is_premium,created_at,updated_at,ban_until) VALUES
    (42,'owner','en','macchiato',0,unixepoch('now')*1000000,unixepoch('now')*1000000,NULL),
    (43,'user','en','macchiato',0,unixepoch('now')*1000000,unixepoch('now')*1000000,NULL);`;
  await helperContainer(['--network','none','--mount',`type=volume,source=${volumeName},target=/state`,
    '--env','YOINK_RUST_DATABASE_PATH=/state/private/state.db','--env',`YOINK_FIXTURE_TOKEN=${fixtureToken}`,
    '--entrypoint','yoink-fixture-server',fixtureImage,'sql'],30000,
    JSON.stringify({token:fixtureToken,sql:`${fixtures}\n${owners}`,mode:'batch'})+'\n');
}

try {
  await mkdir(fixtureDir); await chmod(fixtureDir,0o777);
  await seedDatabase();
  await runtimeMigrate();
  await seedFixtureRows();
  await helperContainer(['--network','none','--entrypoint','ffmpeg','--mount',`type=bind,source=${fixtureDir},target=/fixture`,image,
    '-hide_banner','-loglevel','error','-f','lavfi','-i','color=c=blue:s=160x120:d=1','-f','lavfi','-i','sine=frequency=440:sample_rate=44100:duration=1',
    '-map','0:v:0','-map','1:a:0','-c:v','libx264','-pix_fmt','yuv420p','-c:a','aac','-movflags','+faststart','/fixture/sample.mp4'],20000);
  mediaBytes=await readFile(path.join(fixtureDir,'sample.mp4'));
  assert(mediaBytes.length>100); assert(isoBmffMdatPayloads(mediaBytes).length>0);
  await helperContainer(['--network','none','--entrypoint','ffmpeg','--mount',`type=bind,source=${fixtureDir},target=/fixture`,image,
    '-hide_banner','-loglevel','error','-f','lavfi','-i','color=c=red:s=160x120:d=1','-f','lavfi','-i','sine=frequency=880:sample_rate=44100:duration=1',
    '-map','0:v:0','-map','1:a:0','-c:v','libx264','-pix_fmt','yuv420p','-c:a','aac','-movflags','+faststart','/fixture/red.mp4'],20000);
  await helperContainer(['--network','none','--entrypoint','ffmpeg','--mount',`type=bind,source=${fixtureDir},target=/fixture`,image,
    '-hide_banner','-loglevel','error','-i','/fixture/sample.mp4','-frames:v','1','/fixture/image.png'],15000);
  const playlistMedia = [mediaBytes, await readFile(path.join(fixtureDir,'red.mp4'))];
  const imageBytes = await readFile(path.join(fixtureDir,'image.png'));
  botApi=new BotApiFixture({mediaBytes}); apiPort=await botApi.start();
  sourceFixture=new SourceHttpFixture({mediaBytes,cookieSecret,playlistMedia,imageBytes}); sourcePort=await sourceFixture.start();
  proxyFixture=new AuthenticatedProxyFixture({originPort:sourcePort,authorization:proxyAuth}); proxyPort=await proxyFixture.start();
  appHttpPort=await freeLoopbackPort(); fixturePort=await freeLoopbackPort();
  await launchApp();
  if (scenario === 'all') {
    const sourceUrl = `http://127.0.0.1:${sourcePort}/media?key=${secretMarker}`;
    const replayMessageId = 500;
    botApi.enqueue(42, 42, `/download ${sourceUrl}`, replayMessageId);
    await waitFor(() => botApi.calls.some(call => call.method === 'sendmessage'), 'download admission reply');
    const admissionCall = botApi.calls.find(call => call.method === 'sendmessage');
    assert.equal(admissionCall.data.chat_id, 42);
    const operationId = admissionCall.data.text.match(/[0-9a-f]{8}-[0-9a-f-]{27}/i)?.[0];
    assert.ok(operationId, 'admission reply must contain its operation UUID');

    const succeeded = await waitFor(async () => {
      const row = await queryJob(operationId);
      return row.startsWith('succeeded|') ? row : false;
    }, 'successful media delivery');
    assert.match(succeeded, /^succeeded\|[1-9]\d*\|1$/);
    assert.equal(botApi.calls.filter(call => call.method === 'sendvideo' || call.method === 'senddocument').length, 1, 'successful work must send exactly one Telegram media attachment');
    assert(sourceFixture.mediaRequests >= 1, 'worker must fetch the source fixture');
    const initialSourceRequests = sourceFixture.mediaRequests;
    assert.equal((await docker(['exec', appName, 'sh', '-c', 'test -z "$(ls -A /tmp/yoink-work)" && echo clean'])).trim(), 'clean',
      'workspace must be empty after the acknowledged delivery');

    const originalMediaSendCount = botApi.calls.filter(call => call.method === 'sendvideo' || call.method === 'senddocument').length;
    const replayCallStart = botApi.calls.length;
    botApi.enqueue(42, 42, `/download ${sourceUrl}`, replayMessageId);
    botApi.enqueue(42, 42, `/job ${operationId}`);
    botApi.enqueue(43, 43, `/job ${operationId}`);
    botApi.enqueue(42, -10042, `/download ${sourceUrl}`);
    botApi.enqueue(42, 42, `/cancel ${operationId}`);
    await waitFor(() => botApi.calls.slice(replayCallStart).some(call =>
      call.method === 'sendmessage' && call.data.text.includes(operationId)), 'new existing-operation replay reply');
    await waitFor(() => botApi.calls.slice(replayCallStart).filter(call => call.method === 'sendmessage').length >= 3, 'job and denial replies');
    assert.equal(botApi.calls.filter(call => call.method === 'sendvideo' || call.method === 'senddocument').length, originalMediaSendCount,
      'replayed admission and status/cancel/group commands must not redeliver media');
    assert.equal(sourceFixture.mediaRequests, initialSourceRequests, 'replayed update must not execute another download');
    const replies = botApi.calls.filter(call => call.method === 'sendmessage').map(call => call.data.text);
    assert(replies.some(text => text.includes(operationId) && text.includes('succeeded')), 'owner /job must return the succeeded operation status');
    assert.equal(botApi.calls.filter(call => call.method === 'sendmessage' && call.data.chat_id === 43).length, 0,
      'non-owner is ignored by the private owner gate');
    const unknownId = randomUUID();
    botApi.enqueue(42, 42, `/job ${unknownId}`);
    await waitFor(() => botApi.calls.some(call => call.method === 'sendmessage' && call.data.text === 'Operation not found.'), 'unknown owner operation response');
    assertNoSecret(JSON.stringify(safeTelegramSummary()), 'Telegram replies');
    const logs = await docker(['logs', appName]);
    assertNoSecret(logs, 'application logs');

    const beforeCancelMediaSends = botApi.sendCalls;
    const cancelMessage = 510;
    botApi.enqueue(42, 42, `/download http://127.0.0.1:${sourcePort}/slow?cancel=${cancelMessage}`, cancelMessage);
    const cancelId = await operationForReply(botApi.calls.length);
    await waitFor(() => sourceFixture.slowResponses.size > 0, 'active slow worker request');
    assert((await queryJob(cancelId)).startsWith('running|'), 'operation must be durably running before cancellation');
    const cancelReplyStart = botApi.calls.length;
    botApi.enqueue(42, 42, `/cancel ${cancelId}`);
    await waitFor(async () => (await queryJob(cancelId)).startsWith('cancelled|'), 'cancelled active operation');
    await assertWorkspaceClean();
    assert.equal(botApi.sendCalls, beforeCancelMediaSends, 'cancelled download must not upload');
    sourceFixture.destroySlowResponses();
    await waitFor(() => botApi.calls.slice(cancelReplyStart).some(call => call.method === 'sendmessage'), 'cancel response');

    const requestStart = sourceFixture.requestTimes.length;
    const transientMessage = 520;
    botApi.enqueue(42, 42, `/download http://127.0.0.1:${sourcePort}/transient?case=${transientMessage}`, transientMessage);
    const transientId = await operationForReply(botApi.calls.length);
    const transientResult = await waitFor(async () => {
      const row = await queryJob(transientId);
      return row.startsWith('failed|') ? row : false;
    }, 'transient source failure', 20000);
    assert.match(transientResult, /^failed\|\|3$/);
    const transientTimes = sourceFixture.requestTimes.slice(requestStart);
    assert(transientTimes.length >= 3, '503 must be retried to persisted attempt three');
    assert(transientTimes.at(-1) - transientTimes[0] >= 1000, 'retry interval must be observable at the source');
    assert.equal(botApi.sendCalls, beforeCancelMediaSends, 'transient source failures must not upload');

    botApi.setSendMode('api-error');
    const apiErrorMediaSends = botApi.sendCalls;
    const apiErrorMessage = 530;
    botApi.enqueue(42, 42, `/download ${sourceUrl}&case=${apiErrorMessage}`, apiErrorMessage);
    const apiErrorId = await operationForReply(botApi.calls.length);
    const apiErrorResult = await waitFor(async () => {
      const row = await queryJob(apiErrorId);
      return row.startsWith('failed|') ? row : false;
    }, 'explicit Telegram API failure');
    assert.match(apiErrorResult, /^failed\|\|1$/);
    assert.equal(botApi.sendCalls - apiErrorMediaSends, 1, 'explicit Telegram API failure must be attempted exactly once');

    botApi.setSendMode('rate-limit-once');
    const rateLimitedStart = botApi.calls.length;
    const rateLimitedSends = botApi.sendCalls;
    botApi.enqueue(42, 42, `/download ${sourceUrl}&case=rate-limited`, 532);
    const rateLimitedId = await operationForReply(rateLimitedStart);
    const rateLimitedResult = await waitFor(async () => {
      const row = await queryJob(rateLimitedId);
      return row.startsWith('succeeded|') ? row : false;
    }, 'durable retry after a definitively unsent Telegram rate limit', 20000);
    assert.match(rateLimitedResult, /^succeeded\|\d+\|2$/);
    assert.equal(botApi.sendCalls - rateLimitedSends, 2, 'rate-limited unit must be retried once through a new durable attempt');
    const rateLimitedCalls = botApi.calls.slice(rateLimitedStart).filter(call => call.method === 'sendvideo' || call.method === 'senddocument');
    assert.equal(rateLimitedCalls[0].status, 'rate-limit-once');
    assert.equal(rateLimitedCalls[1].status, 'success');
    assert(rateLimitedCalls[1].receivedAt - rateLimitedCalls[0].receivedAt >= 2000, 'Telegram retry_after must not be shortened to the configured one-second retry delay');

    botApi.setSendMode('api-ambiguous');
    botApi.enqueue(42, 42, `/download ${sourceUrl}&case=api-500`, 535);
    const apiUnknownId = await operationForReply(botApi.calls.length);
    await waitFor(async () => (await queryJob(apiUnknownId)).startsWith('delivery_unknown|'), 'API 500 delivery uncertainty');
    assert.equal(botApi.calls.filter(call => (call.method === 'sendvideo' || call.method === 'senddocument') && call.status === 'api-ambiguous').length, 1);
    botApi.setSendMode('socket-drop');

    const unknownMediaSends = botApi.sendCalls;
    const unknownMessage = 540;
    botApi.enqueue(42, 42, `/download ${sourceUrl}&case=${unknownMessage}`, unknownMessage);
    const deliveryUnknownId = await operationForReply(botApi.calls.length);
    await waitFor(async () => (await queryJob(deliveryUnknownId)).startsWith('delivery_unknown|'), 'ambiguous delivery status');
    const warningStart = botApi.calls.length;
    botApi.enqueue(42, 42, `/job ${deliveryUnknownId}`);
    await waitFor(() => botApi.calls.slice(warningStart).some(call => call.method === 'sendmessage' && call.data.text.includes(deliveryUnknownId) && /could not be confirmed/i.test(call.data.text) && /check the chat before submitting again/i.test(call.data.text)), 'job delivery uncertainty warning');
    await restartApp();
    await sleep(6500);
    assert((await queryJob(deliveryUnknownId)).startsWith('delivery_unknown|'), 'restart must preserve ambiguous delivery state');
    assert((await queryJob(apiUnknownId)).startsWith('delivery_unknown|'), 'restart must preserve API 500 delivery uncertainty');
    assert.equal(botApi.calls.filter(call => (call.method === 'sendvideo' || call.method === 'senddocument') && call.status === 'api-ambiguous').length, 1);
    assert.equal(botApi.sendCalls - unknownMediaSends, 1, 'ambiguous delivery must never be resent after restart');

    const releaseUpload = botApi.holdNextUpload();
    const crashMediaSends = botApi.sendCalls;
    const crashMessage = 550;
    botApi.enqueue(42, 42, `/download ${sourceUrl}&case=${crashMessage}`, crashMessage);
    const crashId = await operationForReply(botApi.calls.length);
    await waitFor(async () => (await queryJob(crashId)).startsWith('delivering|'), 'durable delivering state before crash');
    await waitFor(() => botApi.sendCalls > crashMediaSends, 'blocked media send request');
    await stopApp();
    releaseUpload();
    botApi.setSendMode('socket-drop');
    await launchApp();
    await waitFor(async () => (await queryJob(crashId)).startsWith('delivery_unknown|'), 'expired delivering lease recovery', 15000);
    assert.equal(botApi.sendCalls - crashMediaSends, 1, 'expired in-flight delivery must not be sent again');

    const operationsBeforeBan = Number((await fixtureSql('SELECT count(*) FROM runtime_operations')).trim());
    await fixtureSql("UPDATE users SET ban_until = (unixepoch('now') * 1000000) + 600000000 WHERE id = 42");
    const deniedStart = botApi.calls.length;
    botApi.enqueue(42, 42, `/download ${sourceUrl}&case=ban`, 560);
    await waitFor(() => botApi.calls.slice(deniedStart).some(call => call.method === 'sendmessage' && call.data.text === 'This operation is not allowed.'), 'banned actor denial');
    assert.equal(Number((await fixtureSql('SELECT count(*) FROM runtime_operations')).trim()), operationsBeforeBan, 'banned actor must not create a durable operation');
    await fixtureSql("UPDATE users SET ban_until = NULL WHERE id = 42");

    const mediaSendsBeforeQuota = botApi.sendCalls;
    const slowA = 570;
    const quotaRequestBase = sourceFixture.mediaRequests;
    botApi.enqueue(42, 42, `/download http://127.0.0.1:${sourcePort}/slow?quota=${slowA}`, slowA);
    const quotaA = await operationForReply(botApi.calls.length);
    await waitFor(() => sourceFixture.mediaRequests > quotaRequestBase, 'first quota worker active');
    const slowB = 571;
    botApi.enqueue(42, 42, `/download http://127.0.0.1:${sourcePort}/slow?quota=${slowB}`, slowB);
    const quotaB = await operationForReply(botApi.calls.length);
    const rejectedStart = botApi.calls.length;
    const countBeforeRejection = Number((await fixtureSql('SELECT count(*) FROM runtime_operations')).trim());
    botApi.enqueue(42, 42, `/download ${sourceUrl}&case=quota-reject`, 572);
    await waitFor(() => botApi.calls.slice(rejectedStart).some(call => call.method === 'sendmessage' && call.data.text === 'The download queue is full. Try again later.'), 'per-user quota rejection');
    assert.equal(Number((await fixtureSql('SELECT count(*) FROM runtime_operations')).trim()), countBeforeRejection, 'quota rejection must not persist a third operation');
    assert.equal((await fixtureSql(`SELECT count(*) FROM runtime_operations WHERE id IN ('${quotaA}', '${quotaB}') AND status IN ('queued','running','delivering')`)).trim(), '2');
    botApi.enqueue(42, 42, `/cancel ${quotaA}`);
    botApi.enqueue(42, 42, `/cancel ${quotaB}`);
    await waitFor(async () => (await queryJob(quotaA)).startsWith('cancelled|') && (await queryJob(quotaB)).startsWith('cancelled|'), 'both quota operations cancelled');
    for (const response of sourceFixture.slowResponses) response.destroy();
    await assertWorkspaceClean();
    assert.equal(botApi.sendCalls, mediaSendsBeforeQuota, 'quota rejection and cancellation must not upload');

    botApi.setSendMode('success');
    const mediaSendsBeforeRevocation = botApi.sendCalls;
    botApi.enqueue(42, 42, `/download http://127.0.0.1:${sourcePort}/gated?key=${secretMarker}`, 580);
    const runningRevoked = await operationForReply(botApi.calls.length);
    await waitFor(() => sourceFixture.gatedResponses.size > 0, 'worker blocked on gated source');
    botApi.enqueue(42, 42, `/download ${sourceUrl}&case=revoked-queued`, 581);
    const queuedRevoked = await operationForReply(botApi.calls.length);
    assert((await queryJob(queuedRevoked)).startsWith('queued|'), 'second request must still be queued');
    await fixtureSql("UPDATE users SET ban_until = (unixepoch('now') * 1000000) + 600000000 WHERE id = 42");
    sourceFixture.releaseGatedResponses();
    await waitFor(async () => (await queryJob(runningRevoked)).startsWith('failed|') && (await queryJob(queuedRevoked)).startsWith('failed|'), 'revoked access checked before delivery and before execution');
    assert.equal(sourceFixture.revokedQueuedFetches, 0, 'revoked queued request must not execute its worker');
    assert.equal(botApi.sendCalls, mediaSendsBeforeRevocation, 'revocation before delivery must prevent upload');
    await fixtureSql('UPDATE users SET ban_until = NULL WHERE id = 42');
    await assertWorkspaceClean();

    // Credential selection scenarios run after legacy tests so their counters cannot change legacy baselines.
    const previousNetworkSettingsText = (await fixtureSql("SELECT json_object('proxy_enabled', proxy_enabled, 'proxy_url', proxy_url) FROM dl_user_settings WHERE user_id = 42")).trim();
    const previousNetworkSettings = previousNetworkSettingsText ? JSON.parse(previousNetworkSettingsText) : null;
    await fixtureSql(`INSERT INTO dl_user_settings (user_id, quality, codec, container, proxy_enabled, proxy_url, keyboard, subs_enabled, subs_auto, subs_always_ask, subs_lang, split_size, nsfw_blur, mediainfo, send_as_file, audio_codec, gallery_zip, use_pool_cookies, youtube_auth_mode, dm_topic_thread_id, args_json, created_at, updated_at) VALUES (42, 'best', 'avc1', 'mp4', 1, 'http://${proxyUser}:${proxyPassword}@127.0.0.1:${proxyPort}', '2x3', 0, 0, 0, 'en', 2043000000, 1, 0, 0, 'best', 0, 1, 'cookies', NULL, '{}', unixepoch('now')*1000000, unixepoch('now')*1000000) ON CONFLICT (user_id) DO UPDATE SET proxy_enabled = excluded.proxy_enabled, proxy_url = excluded.proxy_url`);
    const expiration = Math.floor(Date.now() / 1000) + 86400;
    const cookieJar = `# Netscape HTTP Cookie File\n127.0.0.1\tFALSE\t/\tFALSE\t${expiration}\tfixture_auth\t${cookieSecret}\n`;
    let fixtureCookieId = await fixtureSql(`INSERT INTO cookies (user_id, domain, content, is_valid, is_pool, created_at, updated_at) VALUES (42, '127.0.0.1', '${cookieJar.replaceAll("'", "''")}', 1, 0, unixepoch('now')*1000000, unixepoch('now')*1000000) RETURNING id`);
    const protectedUrl = `http://127.0.0.1:${sourcePort}/cookie-media?case=network-selection`;
    const sendsBeforeProtected = botApi.sendCalls;
    const protectedStart = botApi.calls.length;
    botApi.enqueue(42, 42, `/download ${protectedUrl}`, 490);
    const protectedId = await operationForReply(protectedStart);
    const protectedResult = await waitFor(async () => {
      const row = await queryJob(protectedId);
      return row.startsWith('succeeded|') ? row : false;
    }, 'cookie and authenticated proxy media delivery');
    assert.match(protectedResult, /^succeeded\|[1-9]\d*\|1$/);
    assert(proxyFixture.hits > 0 && proxyFixture.authHits > 0, 'worker must use the credential-authenticated constrained proxy');
    assert.equal(botApi.sendCalls - sendsBeforeProtected, 1, 'cookie-protected media must complete one native Telegram upload');
    const protectedOperation = JSON.parse(await fixtureSql(`SELECT request FROM runtime_operations WHERE id = '${protectedId}'`));
    assert.ok(protectedOperation.network?.cookie, 'persisted operation must contain the actual opaque cookie reference');
    assert.ok(protectedOperation.network?.proxy, 'persisted operation must contain the actual opaque proxy reference');
    const protectedOperationText = JSON.stringify(protectedOperation);
    assert(!protectedOperationText.includes(cookieSecret) && !protectedOperationText.includes(proxyPassword) && !protectedOperationText.includes(proxyUser),
      'persisted operation must not contain credential content');
    assertNoSecret(await docker(['logs', appName]), 'application logs after credential-backed delivery');

    // The blocker freezes no cookie and the same proxy; later insertion must not alter its selection.
    sourceFixture.closeGate();
    const sendsBeforeCookieQueue = botApi.sendCalls;
    await fixtureSql(`DELETE FROM cookies WHERE id = ${Number(fixtureCookieId)}`);
    const gatedStart = botApi.calls.length;
    botApi.enqueue(42, 42, `/download http://127.0.0.1:${sourcePort}/gated?case=cookie-reference-blocker`, 492);
    const cookieBlockerId = await operationForReply(gatedStart);
    const blockerOperation = JSON.parse(await fixtureSql(`SELECT request FROM runtime_operations WHERE id = '${cookieBlockerId}'`));
    assert.equal(blockerOperation.network?.cookie, null, 'blocker must freeze a no-cookie selection');
    assert.ok(blockerOperation.network?.proxy, 'blocker must retain the selected proxy reference');
    assert.deepEqual(blockerOperation.network.proxy, protectedOperation.network.proxy, 'blocker must freeze the same proxy reference');
    await waitFor(() => sourceFixture.gatedResponses.size > 0, 'no-cookie proxy blocker worker');
    const protectedFetchesBeforeQueued = sourceFixture.protectedRequests;
    fixtureCookieId = await fixtureSql(`INSERT INTO cookies (user_id, domain, content, is_valid, is_pool, created_at, updated_at) VALUES (42, '127.0.0.1', '${cookieJar.replaceAll("'", "''")}', 1, 0, unixepoch('now')*1000000, unixepoch('now')*1000000) RETURNING id`);
    const queuedCookieStart = botApi.calls.length;
    botApi.enqueue(42, 42, `/download ${protectedUrl}&case=stale-cookie-reference`, 493);
    const staleCookieId = await operationForReply(queuedCookieStart);
    const queuedCookieOperation = JSON.parse(await fixtureSql(`SELECT request FROM runtime_operations WHERE id = '${staleCookieId}'`));
    assert.equal(queuedCookieOperation.network?.cookie?.id, Number(fixtureCookieId), 'queued operation must retain the newly inserted cookie reference');
    assert.deepEqual(queuedCookieOperation.network.proxy, protectedOperation.network.proxy, 'queued operation must retain the selected proxy reference');
    assert((await queryJob(staleCookieId)).startsWith('queued|'), 'selected-cookie operation must be queued before invalidation');
    await fixtureSql(`UPDATE cookies SET is_valid = 0 WHERE id = ${Number(fixtureCookieId)}`);
    sourceFixture.releaseGatedResponses();
    await waitFor(async () => (await queryJob(cookieBlockerId)).startsWith('succeeded|') && (await queryJob(staleCookieId)).startsWith('failed|'), 'queued cookie reference invalidation');
    assert.equal(sourceFixture.protectedRequests, protectedFetchesBeforeQueued, 'stale queued cookie reference must fail without reselection or source fetch');
    assert.equal(botApi.sendCalls - sendsBeforeCookieQueue, 1, 'only the no-cookie blocker may upload; invalidated queued cookie work must not upload');
    sourceFixture.closeGate();

    // Missing proxy must reject with its actionable configuration error, without direct access or upload.
    await fixtureSql("UPDATE dl_user_settings SET proxy_enabled = 1, proxy_url = NULL WHERE user_id = 42");
    const missingProxyBefore = Number((await fixtureSql('SELECT count(*) FROM runtime_operations')).trim());
    const missingProxyFetches = sourceFixture.mediaRequests;
    const missingProxyReplies = botApi.calls.length;
    const missingProxySends = botApi.sendCalls;
    botApi.enqueue(42, 42, `/download http://127.0.0.1:${sourcePort}/media?case=missing-proxy`, 491);
    await waitFor(() => botApi.calls.slice(missingProxyReplies).some(call => call.method === 'sendmessage'
      && call.data.text === 'Proxy is enabled, but no proxy is configured. Configure one or disable proxy use.'), 'missing proxy rejection');
    assert.equal(Number((await fixtureSql('SELECT count(*) FROM runtime_operations')).trim()), missingProxyBefore, 'missing proxy must reject before durable operation creation');
    assert.equal(sourceFixture.mediaRequests, missingProxyFetches, 'missing proxy must not fall back to direct origin access');
    assert.equal(botApi.sendCalls - missingProxySends, 0, 'missing proxy must not upload');

    // Enabling a proxy invalidates frozen direct routes before execution and delivery.
    await fixtureSql('UPDATE dl_user_settings SET proxy_enabled = 0 WHERE user_id = 42');
    sourceFixture.closeGate();
    const lateProxySends = botApi.sendCalls;
    const lateProxyBlockerStart = botApi.calls.length;
    botApi.enqueue(42, 42, `/download http://127.0.0.1:${sourcePort}/gated?case=late-proxy-blocker`, 494);
    const lateProxyBlocker = await operationForReply(lateProxyBlockerStart);
    await waitFor(() => sourceFixture.gatedResponses.size > 0, 'direct worker before proxy enablement');
    const lateProxyHits = proxyFixture.hits;
    const lateProxyQueuedStart = botApi.calls.length;
    botApi.enqueue(42, 42, `/download http://127.0.0.1:${sourcePort}/media?case=late-proxy-queued`, 495);
    const lateProxyQueued = await operationForReply(lateProxyQueuedStart);
    assert((await queryJob(lateProxyQueued)).startsWith('queued|'), 'direct route must be queued before the policy change');
    const directOperation = JSON.parse(await fixtureSql(`SELECT request FROM runtime_operations WHERE id = '${lateProxyQueued}'`));
    assert.equal(directOperation.network.proxy, null, 'queued route must freeze no proxy');
    await fixtureSql(`UPDATE dl_user_settings SET proxy_enabled = 1, proxy_url = 'http://${proxyUser}:${proxyPassword}@127.0.0.1:${proxyPort}' WHERE user_id = 42`);
    sourceFixture.releaseGatedResponses();
    await waitFor(async () => (await queryJob(lateProxyBlocker)).startsWith('failed|') && (await queryJob(lateProxyQueued)).startsWith('failed|'), 'changed proxy policy rejects frozen direct work');
    assert.equal(sourceFixture.lateProxyQueuedFetches, 0, 'new proxy policy must prevent queued direct source access');
    assert.equal(proxyFixture.hits, lateProxyHits, 'frozen direct work must not silently select the new proxy');
    assert.equal(botApi.sendCalls, lateProxySends, 'changed proxy policy must prevent delivery of direct results');
    await assertWorkspaceClean();

    await fixtureSql(`DELETE FROM cookies WHERE id = ${Number(fixtureCookieId)}`);
    if (previousNetworkSettings) {
      const previousUrl = previousNetworkSettings.proxy_url === null ? 'NULL' : `'${previousNetworkSettings.proxy_url.replaceAll("'", "''")}'`;
      await fixtureSql(`UPDATE dl_user_settings SET proxy_enabled = ${previousNetworkSettings.proxy_enabled}, proxy_url = ${previousUrl} WHERE user_id = 42`);
    } else {
      await fixtureSql('DELETE FROM dl_user_settings WHERE user_id = 42');
    }

    // Cache entries survive process restart, but never replace live authorization.
    const cacheUrl = `http://127.0.0.1:${sourcePort}/media?case=cache-reuse&key=${secretMarker}`;
    async function cacheRequest(messageId, terminal = 'succeeded') {
      const start = botApi.calls.length;
      botApi.enqueue(42, 42, `/download ${cacheUrl}`, messageId);
      const id = await operationForReply(start);
      await waitFor(async () => (await queryJob(id)).startsWith(`${terminal}|`), `cache request ${terminal}`);
      return id;
    }
    async function cacheRevision(id) {
      return (await fixtureSql(`SELECT revision FROM runtime_media_cache WHERE cache_key = (SELECT cache_key FROM runtime_operations WHERE id = '${id}') AND expires_at > (unixepoch('now') * 1000000)`)).trim();
    }
    const seededCache = await cacheRequest(800);
    await waitFor(() => cacheRevision(seededCache), 'confirmed upload published as a complete cache entry');
    downloadMaxAttempts = 1;
    await restartApp();
    let cacheSourcesBefore = sourceFixture.mediaRequests;
    let cacheUploadsBefore = botApi.uploadCalls;
    let cacheSendsBefore = botApi.cachedCalls;
    const reusedCache = await cacheRequest(801);
    assert.equal(sourceFixture.mediaRequests, cacheSourcesBefore, 'cache hit must not access the source');
    assert.equal(botApi.uploadCalls, cacheUploadsBefore, 'cache hit must not upload bytes');
    assert.equal(botApi.cachedCalls, cacheSendsBefore + 1, 'cache hit must send a confirmed Telegram file ID');
    assert.equal(await fixtureSql(`SELECT attempt, cache_bypassed FROM runtime_operations WHERE id = '${reusedCache}'`), '1|0');

    botApi.setSendMode('invalid-file-id-once');
    const recoveredCache = await cacheRequest(802);
    assert.equal(botApi.cachedCalls, cacheSendsBefore + 2, 'invalid cached ID must be attempted only once');
    assert.equal(botApi.uploadCalls, cacheUploadsBefore + 1, 'definite invalid ID must fall back to a real upload');
    assert(sourceFixture.mediaRequests > cacheSourcesBefore, 'invalid ID fallback must run the worker');
    assert.equal(await fixtureSql(`SELECT attempt, max_attempts, cache_bypassed FROM runtime_operations WHERE id = '${recoveredCache}'`), '2|2|1', 'single-attempt policy gets exactly one compensating cache attempt');
    assert.equal(await fixtureSql(`SELECT group_concat(attempt || ':' || status, ',') FROM (SELECT attempt, status FROM runtime_delivery_units WHERE operation_id = '${recoveredCache}' ORDER BY attempt, ordinal)`), '1:rejected,2:sent', 'cache fallback preserves rejected and confirmed journals');
    await waitFor(() => cacheRevision(recoveredCache), 'fallback upload replaces the invalid cache revision');

    cacheSourcesBefore = sourceFixture.mediaRequests;
    cacheUploadsBefore = botApi.uploadCalls;
    cacheSendsBefore = botApi.cachedCalls;
    botApi.setSendMode('api-ambiguous');
    const unknownCache = await cacheRequest(803, 'delivery_unknown');
    botApi.setSendMode('success');
    await restartApp();
    assert.equal(await queryJob(unknownCache), 'delivery_unknown||1', 'ambiguous cached send survives restart without requeue');
    assert.equal(sourceFixture.mediaRequests, cacheSourcesBefore, 'ambiguous cached send must not fall back to a worker');
    assert.equal(botApi.uploadCalls, cacheUploadsBefore, 'ambiguous cached send must not upload again');
    assert.equal(botApi.cachedCalls, cacheSendsBefore + 1, 'ambiguous cached send must not repeat');

    sourceFixture.closeGate();
    const cachedBanSends = botApi.sendCalls;
    const cacheBlockerStart = botApi.calls.length;
    botApi.enqueue(42, 42, `/download http://127.0.0.1:${sourcePort}/gated?case=cached-ban-blocker`, 804);
    const cacheBlocker = await operationForReply(cacheBlockerStart);
    await waitFor(() => sourceFixture.gatedResponses.size > 0, 'blocker ahead of cache hit');
    const bannedCacheStart = botApi.calls.length;
    botApi.enqueue(42, 42, `/download ${cacheUrl}`, 805);
    const bannedCache = await operationForReply(bannedCacheStart);
    assert((await queryJob(bannedCache)).startsWith('queued|'));
    await fixtureSql("UPDATE users SET ban_until = (unixepoch('now') * 1000000) + 3600000000 WHERE id = 42");
    sourceFixture.releaseGatedResponses();
    await waitFor(async () => (await queryJob(cacheBlocker)).startsWith('failed|') && (await queryJob(bannedCache)).startsWith('failed|'), 'revoked actor cannot reuse cached media');
    assert.equal(botApi.sendCalls, cachedBanSends, 'cache hit must not bypass revoked authorization');
    await fixtureSql('UPDATE users SET ban_until = NULL WHERE id = 42');

    await fixtureSql(`UPDATE runtime_media_cache SET expires_at = (unixepoch('now') * 1000000) - 1000000 WHERE cache_key = (SELECT cache_key FROM runtime_operations WHERE id = '${seededCache}')`);
    cacheSourcesBefore = sourceFixture.mediaRequests;
    cacheUploadsBefore = botApi.uploadCalls;
    cacheSendsBefore = botApi.cachedCalls;
    const refreshedCache = await cacheRequest(806);
    assert(sourceFixture.mediaRequests > cacheSourcesBefore, 'expired cache must fetch fresh source media');
    assert.equal(botApi.uploadCalls, cacheUploadsBefore + 1);
    assert.equal(botApi.cachedCalls, cacheSendsBefore);
    const beforeDisabled = await waitFor(() => cacheRevision(refreshedCache), 'fresh upload republishes expired cache');
    cacheTtlSeconds = 0;
    await restartApp();
    cacheSourcesBefore = sourceFixture.mediaRequests;
    cacheUploadsBefore = botApi.uploadCalls;
    await cacheRequest(807);
    assert(sourceFixture.mediaRequests > cacheSourcesBefore, 'disabled cache must not reuse stored IDs');
    assert.equal(botApi.uploadCalls, cacheUploadsBefore + 1);
    assert.equal(botApi.cachedCalls, cacheSendsBefore);
    assert.equal(await cacheRevision(refreshedCache), beforeDisabled, 'disabled cache must not publish or refresh entries');
    await assertWorkspaceClean();

  }
  const summaries = [];
  const commandContext = {
    botApi, sourcePort, sourceFixture, fixtureSql, queryJob, waitFor, helperContainer, image, assertWorkspaceClean,
  };
  if (scenario === 'all') {
    summaries.push(await runCommandMediaScenarios(commandContext));
    summaries.push(await runPreferenceScenarios(commandContext));
  }
  downloadTimeoutSeconds = 30;
  cacheTtlSeconds = 3600;
  await restartApp();
  if (scenario === 'all') summaries.push(await runBatchScenarios({ ...commandContext, playlistMedia, imageBytes, restartApp }));
  workerSocketTimeoutSeconds = 20;
  await restartApp();
  if (scenario === 'all') summaries.push(await runProgressScenarios({ ...commandContext, restartApp }));
  if (scenario === 'all' || scenario === 'subtitles') summaries.push(await runSubtitleScenarios({ ...commandContext, restartApp }));
  if (scenario === 'all' || scenario === 'gallery') {
  gallerySource = await startGallerySource({ tempRoot, capture: spawnCapture, helperContainer, image });
  const gallerySummary = await runGalleryScenarios({ ...commandContext, gallerySource,
    proxyUrl: `http://${proxyUser}:${proxyPassword}@127.0.0.1:${proxyPort}`,
    restartGallery: async (enabled, trusted = true) => { galleryEnabled = enabled; galleryTrusted = trusted; proxyFixture.galleryPort = enabled ? gallerySource.port : undefined; await restartApp(); },
    restartApp });
  summaries.push(gallerySummary);
  }
  if (scenario === 'all') summaries.push(await runGroupScenarios({ ...commandContext, playlistMedia, restartGroups: async enabled => { groupsEnabled = enabled; await restartApp(); } }));
  assertNoSecret(await docker(['logs', appContainerId]), 'application logs after failure and credential scenarios');
  assertNoSecret(JSON.stringify(botApi.calls), 'Telegram output after failure scenarios');
  console.log(`Media acceptance (${scenario}) passed. ${summaries.join('. ')}.`);
} catch (error) {
  await reportFailure(error, 'scenario');
  process.exitCode=1;
} finally {
  try { await cleanup(); } catch(error) { await reportFailure(error, 'cleanup'); process.exitCode=1; }
  if(interrupted) process.exitCode=1;
}
