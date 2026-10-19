import assert from 'node:assert/strict';

export async function admissionNotice({ botApi, fixtureSql, waitFor }, from, route = {}) {
  return waitFor(async () => {
    for (const reply of botApi.calls.slice(from)) {
      if (reply.method !== 'sendmessage' || !Number.isSafeInteger(reply.messageId)
        || !Number.isSafeInteger(Number(reply.data.chat_id))
        || (route.chat !== undefined && Number(reply.data.chat_id) !== route.chat)
        || (route.thread !== undefined && reply.data.message_thread_id !== route.thread)) continue;
      if (!['Queued for download.', 'This download was already submitted.', 'Добавлено в очередь загрузки.', 'Эта загрузка уже была отправлена.'].includes(reply.data.text.trim())) continue;
      const replyTo = route.sourceMessageId ?? (typeof reply.data.reply_parameters === 'string'
        ? JSON.parse(reply.data.reply_parameters).message_id : reply.data.reply_parameters?.message_id);
      const sessionTable = route.session === 'cut' ? 'runtime_cut_sessions' : route.session === 'quality' ? 'runtime_quality_sessions' : undefined;
      const operationId = await fixtureSql(`SELECT o.id FROM runtime_operations o LEFT JOIN runtime_progress_notices p ON p.operation_id=o.id WHERE json_extract(o.request,'$.target.bot_id')=123456 AND json_extract(o.request,'$.target.chat_id')=${Number(reply.data.chat_id)} AND (p.message_id=${reply.messageId}${Number.isSafeInteger(replyTo) ? ` OR json_extract(o.request,'$.target.reply_to_message_id')=${replyTo}` : ''}${sessionTable ? ` OR o.id IN (SELECT operation_id FROM ${sessionTable} WHERE bot_id=123456 AND chat_id=${Number(reply.data.chat_id)} AND stage='submitted')` : ''}) ORDER BY o.created_at DESC LIMIT 1`);
      if (operationId) {
        assert(!/[0-9a-f]{8}-[0-9a-f-]{27}/i.test(reply.data.text), 'public admission must not display UUIDs');
        return { operationId, reply };
      }
    }
    return false;
  }, 'ID-free operation admission');
}

export function commandHarness({
  botApi, fixtureSql, queryJob, waitFor, helperContainer, image, assertWorkspaceClean,
}, messageIdStart = 900) {
  let nextMessageId = messageIdStart;
  async function submit(command, { actor = 42, chat = 42, messageId = nextMessageId++, language = 'en', thread, entities, viaBot, replyToBot } = {}) {
    const callStart = botApi.calls.length;
    botApi.enqueue(actor, chat, command, messageId, language, { thread, entities, viaBot, replyToBot });
    const { operationId } = await admissionNotice({ botApi, fixtureSql, waitFor }, callStart, { chat, thread, sourceMessageId: messageId });
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
      call.method === 'sendmessage' && call.data.text === 'Cancellation requested.'),
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
    ], 15000, bytes, { allowEarlyStdinClose: true });
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
  async function execute(label, command, method, mediaExpectation, uploadContract = {}, submissionOptions = {}) {
    botApi.setExpectedUpload(uploadContract.filename
      ? { method, preserveMdat: false, ...uploadContract }
      : null);
    const before = botApi.uploadCalls;
    const sent = await submit(command, submissionOptions);
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
  async function replay(command, original, submissionOptions = {}) {
    const uploads = botApi.uploadCalls;
    const sends = botApi.sendCalls;
    const replayed = await submit(command, { ...submissionOptions, messageId: original.messageId });
    assert.equal(replayed.operationId, original.operationId);
    assert.equal(botApi.uploadCalls, uploads);
    assert.equal(botApi.sendCalls, sends);
  }
  async function progressEdits(operationId) {
    const [chat, message] = (await fixtureSql(`SELECT chat_id || '|' || message_id FROM runtime_progress_notices WHERE operation_id='${operationId}'`)).split('|');
    return botApi.calls.filter(call => call.method === 'editmessagetext'
      && Number(call.data.chat_id) === Number(chat) && Number(call.data.message_id) === Number(message));
  }
  return { submit, succeeded, cancel, inspectUpload, execute, request, reject, replay, progressEdits };
}
