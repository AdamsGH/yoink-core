import test from 'node:test';
import assert from 'node:assert/strict';
import { BotApiFixture } from './bot-api.mjs';
import { isoBmffMdatPayloads } from './media-format.mjs';

function fixtureMedia(payload = Buffer.alloc(128, 7)) {
  const ftyp = Buffer.from([0, 0, 0, 16, 102, 116, 121, 112, 105, 115, 111, 109, 0, 0, 0, 0]);
  const mdatHeader = Buffer.alloc(8);
  mdatHeader.writeUInt32BE(payload.length + 8);
  mdatHeader.write('mdat', 4);
  return Buffer.concat([ftyp, mdatHeader, payload]);
}

test('native upload actions preserve the forum topic', async t => {
  const api = new BotApiFixture({ mediaBytes: fixtureMedia() });
  const port = await api.start();
  t.after(() => api.close());
  for (const action of ['upload_video', 'upload_photo', 'upload_document', 'upload_voice']) {
    const response = await fetch(`http://127.0.0.1:${port}/bottoken/sendChatAction`, {
      method: 'POST', headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ chat_id: -100, message_thread_id: 7, action }),
    });
    assert.equal((await response.json()).result, true);
    assert.equal(api.calls.at(-1).data.message_thread_id, 7);
  }
  assert.equal(api.error, undefined);
});

test('delete retries model lost acknowledgement, not-found and retry-after separately', async t => {
  const api = new BotApiFixture({ mediaBytes: fixtureMedia() });
  const port = await api.start();
  t.after(() => api.close());
  const remove = id => fetch(`http://127.0.0.1:${port}/bottoken/deleteMessage`, {
    method: 'POST', headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ chat_id: 42, message_id: id }),
  });
  api.setDeleteMode('socket-drop-once', 77);
  await assert.rejects(remove(77));
  assert.equal((await (await remove(77)).json()).description, 'Bad Request: message to delete not found');
  api.setDeleteMode('rate-limit-once', 78);
  assert.equal((await (await remove(78)).json()).parameters.retry_after, 2);
  assert.equal((await (await remove(78)).json()).result, true);
  assert.deepEqual(api.calls.map(call => call.status), ['socket-drop-once', 'not-found', 'rate-limit-once', 'success']);
  assert.equal(api.error, undefined);
});

async function postUpload(port, method, filename, mimeType, bytes) {
  const form = new FormData();
  form.set('chat_id', '42');
  form.set(method === 'sendvideo' ? 'video' : 'audio',
    new Blob([bytes], { type: mimeType }), filename);
  const response = await fetch(`http://127.0.0.1:${port}/bottoken/${method}`, { method: 'POST', body: form });
  return { status: response.status, body: await response.json() };
}

test('group fixture preserves topic and reply routing in updates, notices, edits and media', async t => {
  const source = fixtureMedia();
  const api = new BotApiFixture({ mediaBytes: source });
  const port = await api.start();
  t.after(() => api.close());
  const post = async (method, data) => (await fetch(`http://127.0.0.1:${port}/bottoken/${method}`, {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(data),
  })).json();
  api.enqueue(43, -10010, '/help', 10, 'ru', { thread: 18 });
  const update = (await post('getupdates', {})).result[0].message;
  assert.equal(update.message_thread_id, 18);
  assert.equal(update.chat.type, 'supergroup');
  assert.equal(update.from.language_code, 'ru');
  const notice = (await post('sendmessage', { chat_id: -10010, message_thread_id: 18, text: 'queued' })).result;
  assert.equal(notice.message_thread_id, 18);
  const edit = (await post('editmessagetext', { chat_id: -10010, message_id: notice.message_id, text: 'downloading' })).result;
  assert.equal(edit.message_thread_id, 18);
  const form = new FormData();
  form.set('chat_id', '-10010');
  form.set('message_thread_id', '18');
  form.set('reply_parameters', JSON.stringify({ message_id: 10 }));
  form.set('video', new Blob([source], { type: 'video/mp4' }), 'media.mp4');
  const media = await (await fetch(`http://127.0.0.1:${port}/bottoken/sendvideo`, { method: 'POST', body: form })).json();
  assert.equal(media.result.chat.id, -10010);
  assert.equal(media.result.message_thread_id, 18);
  const send = api.calls.at(-1);
  assert.equal(send.chat_id, -10010);
  assert.equal(send.message_thread_id, 18);
  assert.equal(send.reply_to_message_id, 10);
});

test('default video upload retains bounded original media bytes and safe filename', async t => {
  const source = fixtureMedia();
  const api = new BotApiFixture({ mediaBytes: source, maxRequestBytes: 4096 });
  const port = await api.start();
  t.after(() => api.close());
  const response = await postUpload(port, 'sendvideo', 'media.mp4', 'video/mp4', source);
  assert.equal(response.status, 200);
  assert.equal(response.body.ok, true);
  assert.equal(response.body.result.video.mime_type, 'video/mp4');
  assert.equal(response.body.result.video.file_id, 'fixture-video-1');
  assert.deepEqual(isoBmffMdatPayloads(api.lastUploadedBytes), isoBmffMdatPayloads(source));
  assert.deepEqual(api.calls.map(call => call.method), ['sendvideo']);
  assert.equal(api.uploadCalls, 1);
});

test('default contract rejects unsafe filenames, wrong MIME, changed media and oversized requests', async t => {
  const source = fixtureMedia();
  for (const [name, filename, mime, bytes, limit, error] of [
    ['filename', 'secret.mp4', 'video/mp4', source, 4096, /upload filename/],
    ['mime', 'media.mp4', 'audio/mpeg', source, 4096, /upload MIME/],
    ['content', 'media.mp4', 'video/mp4', fixtureMedia(Buffer.alloc(128, 8)), 4096, /preserve source mdat/],
    ['size', 'media.mp4', 'video/mp4', source, 128, /byte bound/],
  ]) {
    await t.test(name, async () => {
      const api = new BotApiFixture({ mediaBytes: source, maxRequestBytes: limit });
      const port = await api.start();
      try {
        await assert.rejects(postUpload(port, 'sendvideo', filename, mime, bytes));
        assert.match(api.error.message, error);
      } finally {
        await api.close();
      }
    });
  }
});

test('Teloxide file parts without an optional MIME header retain content validation', async t => {
  const source = fixtureMedia();
  const api = new BotApiFixture({ mediaBytes: source });
  const port = await api.start();
  t.after(() => api.close());
  const boundary = 'fixture-no-mime';
  const body = Buffer.concat([
    Buffer.from(`--${boundary}\r\nContent-Disposition: form-data; name="chat_id"\r\n\r\n42\r\n--${boundary}\r\nContent-Disposition: form-data; name="video"; filename="media.mp4"\r\n\r\n`),
    source,
    Buffer.from(`\r\n--${boundary}--\r\n`),
  ]);
  const response = await fetch(`http://127.0.0.1:${port}/bottoken/sendvideo`, {
    method: 'POST', headers: { 'content-type': `multipart/form-data; boundary=${boundary}` }, body,
  });
  assert.equal(response.status, 200);
  assert.equal((await response.json()).result.video.mime_type, 'video/mp4');
  assert.deepEqual(api.lastUploadedBytes, source);
});

test('explicit transformed audio contract checks method, filename, MIME type, and response shape', async t => {
  const source = fixtureMedia();
  const transformed = Buffer.alloc(160, 3);
  const api = new BotApiFixture({ mediaBytes: source, maxRequestBytes: 4096 });
  api.setExpectedUpload({ method: 'sendaudio', preserveMdat: false, filename: 'audio.m4a', mimeType: 'audio/mp4' });
  const port = await api.start();
  t.after(() => api.close());
  const response = await postUpload(port, 'sendaudio', 'audio.m4a', 'audio/mp4', transformed);
  assert.equal(response.status, 200);
  assert.equal(response.body.ok, true);
  assert.equal(response.body.result.audio.mime_type, 'audio/mp4');
  assert.equal(response.body.result.audio.file_id, 'fixture-audio-1');
  assert.deepEqual(api.lastUploadedBytes, transformed);
  assert.deepEqual(api.calls.map(call => call.method), ['sendaudio']);
  assert.equal(api.uploadCalls, 1);
});
