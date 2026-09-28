import assert from 'node:assert/strict';

export function commandHarness({
  botApi, fixtureSql, queryJob, waitFor, helperContainer, image, assertWorkspaceClean,
}, messageIdStart = 900) {
  let nextMessageId = messageIdStart;
  async function submit(command, { actor = 42, chat = 42, messageId = nextMessageId++ } = {}) {
    const callStart = botApi.calls.length;
    botApi.enqueue(actor, chat, command, messageId);
    const operationId = await waitFor(() => {
      const reply = botApi.calls.slice(callStart).find(call =>
        call.method === 'sendmessage' && /[0-9a-f]{8}-[0-9a-f-]{27}/i.test(call.data.text));
      return reply?.data.text.match(/[0-9a-f]{8}-[0-9a-f-]{27}/i)?.[0];
    }, `operation admission for ${command.split(' ')[0]}`);
    return { operationId, callStart, messageId };
  }
  async function succeeded(operationId) {
    const row = await waitFor(async () => {
      const value = await queryJob(operationId);
      return value.startsWith('succeeded|') ? value : false;
    }, `command media delivery ${operationId}`, 25000);
    assert.match(row, /^succeeded\|[1-9]\d*\|[1-9]\d*$/);
  }
  async function cancel(operationId) {
    const callStart = botApi.calls.length;
    botApi.enqueue(42, 42, `/cancel ${operationId}`, nextMessageId++);
    await waitFor(() => botApi.calls.slice(callStart).some(call =>
      call.method === 'sendmessage' && call.data.text === `Cancellation requested for operation ${operationId}.`),
    'cancellation acknowledgement');
    await waitFor(async () => (await queryJob(operationId)).startsWith('cancelled|'),
      'cancelled command operation');
  }
  async function inspectUpload(label, {
    video, audio, durationMin, durationMax, height, audioCodec,
  }) {
    const bytes = botApi.lastUploadedBytes;
    assert(Buffer.isBuffer(bytes) && bytes.length > 100 && bytes.length <= 1024 * 1024,
      `${label} upload capture must be present and bounded`);
    const result = await helperContainer([
      '--network', 'none', '--entrypoint', 'ffprobe', image,
      '-v', 'error', '-show_entries', 'format=duration:stream=codec_type,codec_name,height', '-of', 'json', '-i', 'pipe:0',
    ], 15000, bytes);
    const probe = JSON.parse(result);
    const types = probe.streams.map(stream => stream.codec_type);
    assert.equal(types.includes('video'), video, `${label} video stream presence`);
    assert.equal(types.includes('audio'), audio, `${label} audio stream presence`);
    if (durationMin !== undefined || durationMax !== undefined) {
      const duration = Number(probe.format.duration);
      assert(Number.isFinite(duration) && duration >= durationMin && duration <= durationMax,
        `${label} output duration ${duration} must be in [${durationMin}, ${durationMax}]`);
    }
    if (height !== undefined) assert.equal(probe.streams.find(s => s.codec_type === 'video')?.height, height);
    if (audioCodec !== undefined) assert.equal(probe.streams.find(s => s.codec_type === 'audio')?.codec_name, audioCodec);
  }
  async function execute(label, command, method, mediaExpectation, uploadContract = {}) {
    botApi.setExpectedUpload(uploadContract.filename
      ? { method, preserveMdat: false, ...uploadContract }
      : null);
    const before = botApi.uploadCalls;
    const sent = await submit(command);
    await succeeded(sent.operationId);
    assert.equal(botApi.uploadCalls, before + 1, `${label} must perform exactly one real upload`);
    const upload = botApi.calls.slice(sent.callStart).find(call => call.method === method);
    assert(upload, `${label} must use Telegram ${method}`);
    await inspectUpload(label, mediaExpectation);
    await assertWorkspaceClean();
    botApi.setExpectedUpload(null);
    return sent;
  }
  async function request(operationId) {
    return JSON.parse(await fixtureSql(`SELECT request FROM runtime_operations WHERE id = '${operationId}'`));
  }
  async function reject(command, expected, messageId = nextMessageId++) {
    const count = await fixtureSql('SELECT count(*) FROM runtime_operations');
    const callStart = botApi.calls.length;
    botApi.enqueue(42, 42, command, messageId);
    await waitFor(() => botApi.calls.slice(callStart).some(call =>
      call.method === 'sendmessage' && call.data.text.startsWith(expected)), 'rejected command reply');
    assert.equal(await fixtureSql('SELECT count(*) FROM runtime_operations'), count,
      'rejected input must not create an operation');
  }
  async function replay(command, original) {
    const uploads = botApi.uploadCalls;
    const sends = botApi.sendCalls;
    const replayed = await submit(command, { messageId: original.messageId });
    assert.equal(replayed.operationId, original.operationId);
    assert.equal(botApi.uploadCalls, uploads);
    assert.equal(botApi.sendCalls, sends);
  }
  return { submit, succeeded, cancel, inspectUpload, execute, request, reject, replay };
}
