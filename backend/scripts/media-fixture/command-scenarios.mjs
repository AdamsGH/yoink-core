import assert from 'node:assert/strict';

const uploadLimit = 1024 * 1024;

export async function runCommandMediaScenarios({
  botApi,
  sourcePort,
  sourceFixture,
  fixtureSql,
  queryJob,
  waitFor,
  helperContainer,
  image,
  assertWorkspaceClean,
}) {
  const sourceUrl = `http://127.0.0.1:${sourcePort}/media?case=command-acceptance`;
  let nextMessageId = 900;
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
  async function inspectUpload(label, { video, audio, durationMin, durationMax }) {
    const bytes = botApi.lastUploadedBytes;
    assert(Buffer.isBuffer(bytes) && bytes.length > 100 && bytes.length <= uploadLimit,
      `${label} upload capture must be present and bounded`);
    const result = await helperContainer([
      '--network', 'none', '--entrypoint', 'ffprobe', image,
      '-v', 'error', '-show_entries', 'format=duration:stream=codec_type', '-of', 'json', '-i', 'pipe:0',
    ], 15000, bytes);
    const probe = JSON.parse(result);
    const types = probe.streams.map(stream => stream.codec_type);
    if (video) assert(types.includes('video'), `${label} output must contain a video stream`);
    else assert(!types.includes('video'), `${label} output must not contain a video stream`);
    if (audio) assert(types.includes('audio'), `${label} output must contain an audio stream`);
    else assert(!types.includes('audio'), `${label} output must not contain an audio stream`);
    const duration = Number(probe.format.duration);
    assert(Number.isFinite(duration) && duration >= durationMin && duration <= durationMax,
      `${label} output duration ${duration} must be in [${durationMin}, ${durationMax}]`);
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

  try {
    const video = await execute('video', `/video ${sourceUrl}`, 'sendvideo',
      { video: true, audio: true, durationMin: 0.8, durationMax: 1.5 });
    const videoRequest = JSON.parse(await fixtureSql(
      `SELECT request FROM runtime_operations WHERE id = '${video.operationId}'`));
    assert.equal(videoRequest.media_mode, 'video', 'video command must persist video mode');
    const videoSends = botApi.calls.filter(call => call.method === 'sendvideo').length;
    const replayCallStart = botApi.calls.length;
    botApi.enqueue(42, 42, `/video ${sourceUrl}`, video.messageId);
    await waitFor(() => botApi.calls.slice(replayCallStart).some(call =>
      call.method === 'sendmessage' && call.data.text.includes(video.operationId)),
    'new same-message existing-operation replay reply');
    assert.equal(botApi.calls.filter(call => call.method === 'sendvideo').length, videoSends,
      'same Telegram message replay must not upload a second video');
    assert((await queryJob(video.operationId)).startsWith('succeeded|'),
      'same-message replay must retain the original operation');
    assert.equal(await fixtureSql(
      `SELECT count(*) FROM runtime_operations WHERE json_extract(request, '$.target.reply_to_message_id') = ${video.messageId}`),
    '1', 'same Telegram message must map to exactly one persisted operation');

    const audio = await execute('audio', `/audio ${sourceUrl}`, 'sendaudio',
      { video: false, audio: true, durationMin: 0.8, durationMax: 1.5 },
      { filename: 'audio.m4a', mimeType: 'audio/mp4' });
    const audioRequest = JSON.parse(await fixtureSql(
      `SELECT request FROM runtime_operations WHERE id = '${audio.operationId}'`));
    assert.equal(audioRequest.media_mode, 'audio', 'audio command must persist audio mode');

    const cut = await execute('cut', `/cut ${sourceUrl} 0.2 0.7`, 'sendvideo',
      { video: true, audio: true, durationMin: 0.35, durationMax: 0.8 },
      { filename: 'media.mp4', mimeType: 'video/mp4' });
    const cutRequest = JSON.parse(await fixtureSql(
      `SELECT request FROM runtime_operations WHERE id = '${cut.operationId}'`));
    assert.equal(cutRequest.media_mode, 'video', 'cut request must persist video mode');
    assert.deepEqual(cutRequest.clip_range, { start_seconds: 0.2, end_seconds: 0.7 },
      'cut request must persist its requested clip range');

    sourceFixture.closeGate();
    const blocker = await submit(`/download http://127.0.0.1:${sourcePort}/gated?case=admission-blocker`);
    await waitFor(() => sourceFixture.gatedResponses.size > 0, 'admission-only worker blocker');

    const playlist = await submit(`/playlist 1-2 ${sourceUrl}`);
    const playlistRequest = JSON.parse(await fixtureSql(
      `SELECT request FROM runtime_operations WHERE id = '${playlist.operationId}'`));
    assert.equal(playlistRequest.params.engine, 'yt_dlp', 'playlist command must select yt-dlp');
    assert.deepEqual(playlistRequest.params.playlist, { start: 1, end: 2 },
      'playlist range must persist exactly');
    await cancel(playlist.operationId);

    const gallery = await submit(`/image ${sourceUrl}`);
    const galleryRequest = JSON.parse(await fixtureSql(
      `SELECT request FROM runtime_operations WHERE id = '${gallery.operationId}'`));
    assert.equal(galleryRequest.media_mode, 'image', 'image command must persist image mode');
    assert.equal(galleryRequest.params.engine, 'gallery', 'image command must select Gallery');
    await cancel(gallery.operationId);

    const selection = await submit(`/audio 2 ${sourceUrl}&t=60`);
    const selectionRequest = JSON.parse(await fixtureSql(
      `SELECT request FROM runtime_operations WHERE id = '${selection.operationId}'`));
    assert.deepEqual(selectionRequest.params.playlist, { start: 2, end: 2 },
      'timestamped URLs must not reinterpret an explicit playlist selector as a clip');
    assert.equal(selectionRequest.clip_range, null);
    await cancel(selection.operationId);
    await cancel(blocker.operationId);
    sourceFixture.releaseGatedResponses();
    await assertWorkspaceClean();

    const countBeforeInvalid = Number((await fixtureSql('SELECT count(*) FROM runtime_operations')).trim());
    const invalidStart = botApi.calls.length;
    botApi.enqueue(42, 42, `/cut ${sourceUrl} 0 invalid`);
    await waitFor(() => botApi.calls.slice(invalidStart).some(call => call.method === 'sendmessage' && call.data.text.startsWith('Usage:')),
      'malformed cut response');
    assert.equal(Number((await fixtureSql('SELECT count(*) FROM runtime_operations')).trim()), countBeforeInvalid,
      'malformed command input must not be admitted');

    const gateStart = botApi.calls.length;
    const beforeDenied = Number((await fixtureSql('SELECT count(*) FROM runtime_operations')).trim());
    botApi.enqueue(43, 43, `/video ${sourceUrl}`);
    botApi.enqueue(42, -10042, `/audio ${sourceUrl}`);
    botApi.enqueue(42, 42, `/video@another_bot ${sourceUrl}`);
    botApi.enqueue(42, 42, '/status');
    const barrierReply = await waitFor(() => botApi.calls.slice(gateStart).find(call =>
      call.method === 'sendmessage' && call.data.text.startsWith('Yoink ')), 'owner status barrier reply');
    assert.equal(botApi.calls.slice(gateStart).filter(call => call.method === 'sendmessage').length, 1,
      'denied commands must produce no reply before or beyond the distinct status barrier');
    assert.equal(barrierReply.data.chat_id, 42);
    assert.equal(Number((await fixtureSql('SELECT count(*) FROM runtime_operations')).trim()), beforeDenied,
      'denied command forms must not create operations');
    await assertWorkspaceClean();
    return 'command media acceptance passed; playlist and Gallery assertions cover admission and persisted parameters only, with no live provider integration asserted.';
  } finally {
    botApi.setExpectedUpload(null);
  }
}
