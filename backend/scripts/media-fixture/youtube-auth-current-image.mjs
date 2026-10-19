import assert from 'node:assert/strict';
import { commandHarness, admissionNotice } from './command-harness.mjs';
import { createCookieBrowserProviderControl } from './cookie-browser-provider-control.mjs';

const sql = value => `'${String(value).replaceAll("'", "''")}'`;
export function assertCurrentImageHooks(context) {
  for (const name of ['syntheticControl', 'refreshGate', 'restartYoutubeAuth', 'restartYoutubeAuthServices', 'restartYoutubeAuthGateway', 'syntheticPotControl', 'gatewayRequest', 'runSyntheticPotWorker']) {
    assert.equal(typeof context[name], 'function', `current-image acceptance requires ${name}`);
  }
}

// Hooks operate only on coordinator-owned disposable topology and existing settings.
export async function runYoutubeAuthCurrentImage(context, { cookieId, credential, assertEvidence, assertRequest }) {
  assertCurrentImageHooks(context);
  const { fixtureSql, botApi, waitFor, queryJob, syntheticControl, refreshGate,
    restartYoutubeAuth, restartYoutubeAuthServices, restartApp, assertWorkspaceClean } = context;
  const h = commandHarness(context, 3_200_000);
  const providers = createCookieBrowserProviderControl(fixtureSql);
  const user = JSON.parse(await fixtureSql("SELECT json_object('role',role,'ban_until',ban_until) FROM users WHERE id=42"));
  let serial = 10;
  const command = () => `/download https://youtu.be/Synthetic${String(serial++).padStart(2, '0')}`;
  const setCredential = async expiry => fixtureSql(`UPDATE cookies SET content=${sql(credential(expiry))},is_valid=1,updated_at=updated_at+1 WHERE id=${cookieId}`);
  const restoreUser = () => fixtureSql(`UPDATE users SET role=${sql(user.role)},ban_until=${user.ban_until === null ? 'NULL' : user.ban_until} WHERE id=42`);
  const noDelivery = async (sent, before) => {
    await waitFor(async () => (await queryJob(sent.operationId)).startsWith('failed|'), 'OAuth operation fails closed', 120000);
    assert.equal(botApi.sendCalls, before);
    assert.equal(await fixtureSql(`SELECT count(*) FROM runtime_delivery_units WHERE operation_id=${sql(sent.operationId)}`), '0');
    assert.equal(await fixtureSql(`SELECT count(*) FROM runtime_media_cache WHERE cache_key=(SELECT cache_key FROM runtime_operations WHERE id=${sql(sent.operationId)})`), '0');
    assertEvidence(await fixtureSql(`SELECT * FROM runtime_operations WHERE id=${sql(sent.operationId)}`));
    await assertWorkspaceClean();
  };
  try {
    await providers.enable();
    await providers.set({ oauth: 'ok', account: 'valid' });
    await restartYoutubeAuth({ maxBytes: 65536, maxItems: 1, processingTimeoutSeconds: 60 });
    // Natural expiry keeps content/version unchanged. Both running and queued
    // references must fail without refreshing an already admitted credential.
    for (const mutation of ['expiry', 'ban', 'version']) {
      const expiry = mutation === 'expiry' ? new Date(Date.now() + 40000).toISOString() : '2099-01-01T00:00:00Z';
      await setCredential(expiry);
      const before = botApi.sendCalls, counts = await providers.counts();
      await syntheticControl('arm', 'download');
      const running = await h.submit(command());
      await waitFor(async () => (await syntheticControl('state')).entered, 'held OAuth processing');
      const queued = await h.submit(command());
      assert((await queryJob(queued.operationId)).startsWith('queued|'));
      const frozenRunning = await h.request(running.operationId), frozenQueued = await h.request(queued.operationId);
      assertRequest(frozenRunning, cookieId); assertRequest(frozenQueued, cookieId);
      if (mutation === 'expiry') await waitFor(() => Date.now() > Date.parse(expiry) + 1000, 'admitted credential natural expiry', 60000);
      if (mutation === 'ban') await fixtureSql('UPDATE users SET ban_until=4102444800000000 WHERE id=42');
      if (mutation === 'version') await fixtureSql(`UPDATE cookies SET updated_at=updated_at+1 WHERE id=${cookieId}`);
      await syntheticControl('release');
      await noDelivery(running, before);
      await noDelivery(queued, before);
      assert.deepEqual(await h.request(running.operationId), frozenRunning);
      assert.deepEqual(await h.request(queued.operationId), frozenQueued);
      assert.equal((await providers.counts()).refresh, counts.refresh, 'admitted references never refresh');
      await restoreUser();
    }
    // The fixture provider returns immediately unless the coordinator's
    // cfg(test-fixtures) gate holds exchange outside the database guard.
    for (const mutation of ['none', 'version', 'ban', 'mode', 'error']) {
      await setCredential('2000-01-01T00:00:00Z');
      await providers.set({ oauth: mutation === 'error' ? 'error' : 'ok', account: 'valid' });
      const before = botApi.sendCalls, count = await fixtureSql('SELECT count(*) FROM runtime_operations');
      const calls = botApi.calls.length, messageId = 3_300_000 + serial++;
      const refreshBefore = (await providers.counts()).refresh;
      await refreshGate('arm');
      botApi.enqueue(42, 42, command(), messageId);
      await waitFor(async () => (await refreshGate('state')).entered, 'refresh exchange gate');
      if (mutation === 'version') await fixtureSql(`UPDATE cookies SET updated_at=updated_at+1 WHERE id=${cookieId}`);
      if (mutation === 'ban') await fixtureSql('UPDATE users SET ban_until=4102444800000000 WHERE id=42');
      if (mutation === 'mode') await fixtureSql("UPDATE dl_user_settings SET youtube_auth_mode='cookies' WHERE user_id=42");
      const heldContent = await fixtureSql(`SELECT content FROM cookies WHERE id=${cookieId}`);
      const heldRow = await fixtureSql(`SELECT json_object('content',content,'updated_at',updated_at,'is_valid',is_valid) FROM cookies WHERE id=${cookieId}`);
      await refreshGate('release');
      if (mutation === 'none') {
        const { operationId: id } = await admissionNotice(context, calls);
        await h.succeeded(id);
        assert.equal(botApi.sendCalls, before + 1);
        assert((await fixtureSql(`SELECT content FROM cookies WHERE id=${cookieId}`)) !== heldContent, 'successful refresh must replace the expired credential');
        assertRequest(await h.request(id), cookieId);
      } else {
        await waitFor(() => botApi.calls.slice(calls).some(c => c.method === 'sendmessage' && Number(c.data.chat_id) === 42), 'refresh race rejection');
        assert.equal(await fixtureSql('SELECT count(*) FROM runtime_operations'), count);
        assert.equal(botApi.sendCalls, before);
        assert((await fixtureSql(`SELECT json_object('content',content,'updated_at',updated_at,'is_valid',is_valid) FROM cookies WHERE id=${cookieId}`)) === heldRow, 'failed refresh CAS must not overwrite the credential row');
      }
      assert.equal((await providers.counts()).refresh, refreshBefore + 1);
      assertEvidence(botApi.calls.slice(calls));
      await restoreUser();
      await fixtureSql("UPDATE dl_user_settings SET youtube_auth_mode='oauth' WHERE user_id=42");
    }
    await providers.set({ oauth: 'ok', account: 'valid' });
    await setCredential('2099-01-01T00:00:00Z');
    for (const failure of ['bytes', 'deadline', 'unsupported-playlist']) {
      await restartYoutubeAuth({ maxBytes: failure === 'bytes' ? 1024 : 65536, maxItems: 1, processingTimeoutSeconds: failure === 'deadline' ? 2 : 60 });
      const before = botApi.sendCalls;
      if (failure === 'deadline') await syntheticControl('arm', 'download');
      const sent = await h.submit(failure === 'unsupported-playlist' ? '/playlist 1-2 https://youtu.be/Synthetic99' : command());
      if (failure === 'deadline') await waitFor(async () => (await syntheticControl('state')).entered, 'deadline upstream reached');
      await noDelivery(sent, before);
      await syntheticControl('reset');
    }
    await restartYoutubeAuth({ maxBytes: 65536, maxItems: 1, processingTimeoutSeconds: 60 });
    botApi.setSendMode('socket-drop');
    const before = botApi.sendCalls, uncertainCommand = command();
    const uncertain = await h.submit(uncertainCommand);
    await waitFor(async () => (await queryJob(uncertain.operationId)).startsWith('delivery_unknown|'), 'OAuth uncertain delivery');
    const journal = await fixtureSql(`SELECT * FROM runtime_delivery_units WHERE operation_id=${sql(uncertain.operationId)}`);
    assert.equal(await fixtureSql(`SELECT count(*) FROM runtime_delivery_units WHERE operation_id=${sql(uncertain.operationId)} AND status='unknown' AND artifact_count=1 AND message_ids IS NULL`), '1');
    assertEvidence(journal);
    assertRequest(await h.request(uncertain.operationId), cookieId);
    assert.equal(await fixtureSql(`SELECT count(*) FROM runtime_media_cache WHERE cache_key=(SELECT cache_key FROM runtime_operations WHERE id=${sql(uncertain.operationId)})`), '0');
    assert.equal(botApi.sendCalls, before + 1);
    await restartYoutubeAuthServices();
    await restartApp();
    botApi.setSendMode('success');
    await h.replay(uncertainCommand, uncertain);
    assert((await queryJob(uncertain.operationId)).startsWith('delivery_unknown|'));
    assert.equal(await fixtureSql(`SELECT * FROM runtime_delivery_units WHERE operation_id=${sql(uncertain.operationId)}`), journal);
    assert.equal(botApi.sendCalls, before + 1);
    const recovered = await h.submit(command());
    await h.succeeded(recovered.operationId);
    assert.equal(botApi.sendCalls, before + 2, 'paired service restart permits fresh OAuth work');
    await runSyntheticPotGateway(context);
    assertEvidence(botApi.calls);
    await assertWorkspaceClean();
    return 'current-image synthetic OAuth expiry/rechecks, refresh CAS, byte/deadline/unsupported-playlist, uncertain delivery and paired service restart';
  } finally {
    botApi.setSendMode('success');
    const cleanup = await Promise.allSettled([syntheticControl('reset'), refreshGate('reset'), restoreUser(), providers.restore()]);
    const failures = cleanup.filter(result => result.status === 'rejected').map(result => result.reason);
    if (failures.length) throw new AggregateError(failures, 'current-image fixture cleanup failed');
  }
}

export async function runSyntheticPotGateway({ gatewayRequest, syntheticPotControl, runSyntheticPotWorker, restartYoutubeAuthServices, restartYoutubeAuthGateway, waitFor }) {
  const payload = { content_binding: 'abcdefghijk', innertube_context: { client: { clientName: 'WEB' } }, bypass_cache: false, challenge: null, proxy: null, source_address: null, disable_tls_verification: false };
  const request = (route, value = {}) => gatewayRequest(route, value, { timeoutMs: 5000 });
  try {
    const ping = await request('/pot/ping');
    assert.equal(ping.status, 200);
    assert.equal(ping.body.version, '2.0.0');
    const token = await request('/pot', payload);
    assert.equal(token.status, 200);
    assert.equal(token.body.poToken, 'synthetic-pot');
    // Must execute the baked Python provider and pinned yt-dlp API, not a
    // parallel Node imitation. The coordinator supplies its container probe.
    assert.equal(await runSyntheticPotWorker(), true);
    for (let i = 0; i < 3; i++) assert.equal((await request('/pot', { ...payload, proxy: 'http://invalid.example' })).status, 502);
    assert.equal((await request('/pot/ping')).status, 200, 'local validation never poisons admission');
    for (let i = 1; i <= 2; i++) {
      await syntheticPotControl('arm');
      // Request resolves only after its bounded client disconnect. Never leave
      // an unjoined request promise running across restart or cleanup.
      const held = gatewayRequest('/pot', payload, { timeoutMs: 1000 }).then(value => value, () => ({ transportError: true }));
      let result;
      try { await waitFor(async () => (await syntheticPotControl('state')).active === i, 'synthetic PoToken dispatched work'); }
      finally { result = await held; }
      assert.equal(result.transportError, true, 'client must disconnect before provider response');
    }
    assert.equal((await request('/pot/ping')).status, 503, 'two uncertain PoToken completions exhaust gateway slots');
    assert.equal((await syntheticPotControl('state')).active, 2, 'disconnect is not provider cancellation');
    await restartYoutubeAuthGateway();
    assert.equal((await syntheticPotControl('state')).active, 2, 'gateway-only restart does not cancel provider work');
    assert.equal((await request('/pot', payload)).status, 502, 'provider remains full after gateway-only restart');
    await restartYoutubeAuthServices();
    assert.equal((await syntheticPotControl('state')).active, 0, 'paired restart resets synthetic provider jobs');
    assert.equal((await request('/pot', payload)).status, 200);
  } finally {
    await syntheticPotControl('reset');
  }
}
