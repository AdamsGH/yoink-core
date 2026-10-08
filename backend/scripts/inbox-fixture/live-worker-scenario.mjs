import assert from 'node:assert/strict';
import { heldWorkerEvidence } from './dns-harness.mjs';

export async function liveWorkerShutdown(c) {
  await c.waitFor(async () => (await c.db.query("SELECT count(*) FROM inbox_jobs WHERE state IN ('ready','running')")) === '0', 'drain before actual anonymous worker admission');
  await c.waitFor(async () => (await c.db.query("SELECT count(*) FROM inbox_notifications WHERE state IN ('ready','claimed','sending')")) === '0', 'drain completion effects before live worker gate');
  await c.stop(); await c.workspaceClean();
  const dns = await c.startDns();
  const url = `https://${dns.hostname}/shutdown`;
  let item, terminal = false;
  try {
    await c.launch({ dns: dns.address }); await c.ready();
    // Fail before admission if Docker did not install the per-container resolver.
    await c.observeDnsWorker(dns.address);
    const token = await c.login(42);
    const botEffects = () => c.bot.calls.filter(call => ['sendmessage', 'editmessagetext'].includes(call.method)).length;
    const effectsBefore = botEffects();
    item = await c.request('/api/v1/inbox/items', { token, method: 'POST', body: { url }, expected: 201 });
    assert(Number.isSafeInteger(item.id) && item.id > 0);
    const worker = await c.waitFor(async () => {
      const state = await dns.state();
      if (!state.records.some(record => record.name === dns.hostname && record.held && !record.replied)) return false;
      return heldWorkerEvidence(await c.observeDnsWorker(dns.address), state, dns.hostname);
    }, 'actual confined Python child owns unanswered DNS query socket', 10000);
    const claim = await c.db.query(`SELECT state,attempts,length(coalesce(lease_token,'')) FROM inbox_jobs WHERE item_id=${item.id} AND kind='enrich'`);
    assert.match(claim, /^running\|1\|[1-9]\d*$/);
    assert.equal(await c.db.query(`SELECT status FROM inbox_items WHERE id=${item.id}`), 'pending');
    assert.equal(await c.db.query(`SELECT count(*) FROM inbox_notifications WHERE item_id=${item.id}`), '0');
    // Collect again immediately before SIGTERM; stale PID/query observations
    // cannot become a pass. Original fixture_main also snapshots live children
    // at the actual signal boundary before runtime cancellation.
    const boundary = heldWorkerEvidence(await c.observeDnsWorker(dns.address), await dns.state(), dns.hostname);
    assert.equal(boundary?.pid, worker.pid);
    console.log(`EVIDENCE live confined-worker pid=${worker.pid}, private workspace mode=0700, owned DNS socket unanswered`);
    await c.stop({ liveWorker: true }); await c.workspaceClean();
    assert.equal(await c.db.query(`SELECT state,attempts,coalesce(lease_token,''),coalesce(lease_until,''),coalesce(last_error,'') FROM inbox_jobs WHERE item_id=${item.id} AND kind='enrich'`), 'ready|0|||', 'owning SIGTERM restores the consumed attempt and releases both lease fields without failure');
    assert.equal(await c.db.query(`SELECT status FROM inbox_items WHERE id=${item.id}`), 'pending');
    assert.equal((await dns.state()).phase, 'holding', 'DNS was not released to help the original worker exit');
    const beforeRestartDns = (await dns.release()).records.length;
    await c.launch({ dns: dns.address }); await c.ready();
    await c.waitFor(async () => (await c.db.query(`SELECT state FROM inbox_jobs WHERE item_id=${item.id} AND kind='enrich'`)) === 'failed', 'restart actual worker reaches terminal NXDOMAIN denial without exemptions');
    terminal = true;
    assert.equal(await c.db.query(`SELECT attempts,coalesce(lease_token,''),coalesce(lease_until,'') FROM inbox_jobs WHERE item_id=${item.id} AND kind='enrich'`), '1||');
    assert.equal(await c.db.query(`SELECT status,crawl_status FROM inbox_items WHERE id=${item.id}`), 'failed|failed');
    assert.equal(await c.db.query(`SELECT count(*) FROM inbox_jobs WHERE item_id=${item.id} AND kind='enrich'`), '1', 'restart reuses the admitted durable job');
    assert.equal(await c.db.query(`SELECT count(*) FROM inbox_jobs WHERE item_id=${item.id} AND kind='classify'`), '0');
    assert.equal(await c.db.query(`SELECT count(*) FROM inbox_notifications WHERE item_id=${item.id}`), '0');
    assert.equal(await c.db.query(`SELECT count(*) FROM inbox_items WHERE user_id=42 AND url='${url}'`), '1');
    assert((await dns.state()).records.slice(beforeRestartDns).some(record => record.name === dns.hostname && !record.held && record.replied && record.rcode === 3), 'restarted real worker made a new DNS operation answered NXDOMAIN with zero addresses');
    await c.workspaceClean();
    const barrier = await c.request('/api/v1/inbox/items', { token, method: 'POST', body: { url: 'https://github.com/fixture/live-worker-restart-barrier' }, expected: 201 });
    assert(Number.isSafeInteger(barrier.id) && barrier.id > 0);
    await c.waitFor(async () => (await c.db.query(`SELECT status FROM inbox_items WHERE id=${barrier.id}`)) === 'classified', 'later native Content/Insight job completes after worker reap/restart');
    assert.equal(await c.db.query(`SELECT state,attempts FROM inbox_jobs WHERE item_id=${item.id} AND kind='enrich'`), 'failed|1', 'terminal admission does not dispatch again');
    assert.equal(botEffects(), effectsBefore, 'HTTP-created failed admission emits no duplicate Telegram effects across restart');
  } finally {
    // The runtime pins default recovery launches to this resolver until close.
    // Even a failed observation must not replay its queued public URL using host
    // DNS. Reach a real terminal outcome under NXDOMAIN replies before unpinning.
    await c.stop();
    // A lost HTTP response may still have committed admission. Read the actual
    // owned Store row before deciding that it is safe to remove its resolver.
    const savedId = await c.db.query(`SELECT id FROM inbox_items WHERE user_id=42 AND url='${url}'`);
    if (savedId) {
      assert(/^\d+$/.test(savedId) && Number.isSafeInteger(Number(savedId)) && Number(savedId) > 0);
      item = { id: Number(savedId) };
      terminal = (await c.db.query(`SELECT state FROM inbox_jobs WHERE item_id=${item.id} AND kind='enrich'`)) === 'failed';
    }
    await dns.release();
    if (item && !terminal) {
      await c.launch({ dns: dns.address }); await c.ready();
      await c.waitFor(async () => (await c.db.query(`SELECT state FROM inbox_jobs WHERE item_id=${item.id} AND kind='enrich'`)) === 'failed', 'safe real denial after failed live-worker observation');
      await c.stop();
    }
    await dns.close();
    await c.workspaceClean();
    await c.launch(); await c.ready();
  }
}
