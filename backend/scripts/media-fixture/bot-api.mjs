import assert from 'node:assert/strict';
import http from 'node:http';
import { isoBmffMdatPayloads } from './media-format.mjs';

export class BotApiFixture {
  #server;
  #error;
  #updates = [];
  #updateId = 1;
  #nextMessageId = 1000;
  #calls = [];
  #sendMode = 'success';
  #sendGate;
  #fileIds = new Set();
  #sendCalls = 0;
  #uploadCalls = 0;
  #cachedCalls = 0;
  #expectedUpload = null;
  #lastUploadedBytes;

  constructor({ mediaBytes, maxRequestBytes = 3 * 1024 * 1024 }) {
    this.mediaBytes = mediaBytes;
    this.maxRequestBytes = maxRequestBytes;
  }
  get error() { return this.#error; }
  get calls() { return this.#calls; }
  get sendCalls() { return this.#sendCalls; }
  get uploadCalls() { return this.#uploadCalls; }
  get cachedCalls() { return this.#cachedCalls; }
  get sendMode() { return this.#sendMode; }
  get lastUploadedBytes() { return this.#lastUploadedBytes; }
  setExpectedUpload(contract) {
    if (contract !== null) {
      assert(contract && typeof contract.method === 'string', 'expected upload contract needs a Telegram method');
      assert.equal(typeof contract.preserveMdat, 'boolean', 'expected upload contract needs an explicit mdat policy');
      assert.equal(typeof contract.filename, 'string', 'expected upload contract needs a filename');
      assert.equal(typeof contract.mimeType, 'string', 'expected upload contract needs a MIME type');
    }
    this.#expectedUpload = contract;
  }
  setSendMode(mode) { this.#sendMode = mode; }
  holdNextUpload() {
    let release;
    this.#sendGate = new Promise(resolve => { release = resolve; });
    this.#sendMode = 'hold';
    return () => { release(); this.#sendGate = undefined; };
  }
  enqueue(actor, chat, text, id) {
    this.#updates.push({ update_id: this.#updateId++, message: this.#message(actor, chat, text, id) });
  }
  async start() {
    this.#server = http.createServer((req, res) => {
      void this.#handle(req, res).catch(error => {
        this.#error ??= error;
        res.destroy(error);
      });
    });
    await new Promise((resolve, reject) => {
      this.#server.once('error', reject);
      this.#server.listen(0, '127.0.0.1', () => resolve());
    });
    return this.#server.address().port;
  }
  async close() {
    if (!this.#server) return;
    this.#server.closeAllConnections?.();
    await new Promise((resolve, reject) => this.#server.close(error => error ? reject(error) : resolve()));
  }
  async #handle(req, res) {
    const method = (req.url.split('/').at(-1) ?? '').toLowerCase();
    const chunks = [];
    let requestLength = 0;
    for await (const chunk of req) {
      requestLength += chunk.length;
      assert(requestLength <= this.maxRequestBytes, 'Telegram fixture request exceeds its byte bound');
      chunks.push(chunk);
    }
    const body = Buffer.concat(chunks);
    res.setHeader('content-type', 'application/json');
    if (method === 'getme') {
      res.end(JSON.stringify({ ok: true, result: { id: 123456, is_bot: true, first_name: 'Fixture', username: 'yoink_fixture_bot', can_join_groups: false, can_read_all_group_messages: false, supports_inline_queries: false, has_main_web_app: false } }));
    } else if (method === 'deletewebhook') {
      res.end(JSON.stringify({ ok: true, result: true }));
    } else if (method === 'getwebhookinfo') {
      res.end(JSON.stringify({ ok: true, result: { url: '', has_custom_certificate: false, pending_update_count: 0, allowed_updates: ['message'] } }));
    } else if (method === 'getupdates') {
      const batch = this.#updates;
      this.#updates = [];
      res.end(JSON.stringify({ ok: true, result: batch }));
    } else if (method === 'sendmessage') {
      const data = JSON.parse(body.toString('utf8'));
      this.#calls.push({ method, data });
      res.end(JSON.stringify({ ok: true, result: this.#message(123456, Number(data.chat_id), data.text) }));
    } else if (method === 'sendvideo' || method === 'senddocument' || method === 'sendaudio') {
      await this.#handleMedia(method, req, res, body);
    } else {
      res.writeHead(404);
      res.end(JSON.stringify({ ok: false, error_code: 404, description: 'unexpected fixture API method' }));
    }
  }
  async #handleMedia(method, req, res, body) {
    this.#sendCalls++;
    const boundary = String(req.headers['content-type'] ?? '').match(/boundary=([^;]+)/)?.[1];
    let data;
    let filePart;
    let uploadedMimeType;
    if (boundary) {
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
      const fileParts = parts.filter(part => /filename="/.test(part.headers));
      assert(fileParts.length <= 1, 'Telegram upload must contain at most one file');
      filePart = fileParts[0];
      assert(parts.every(part => !/filename=/.test(part.headers) || part === filePart), 'unexpected upload filename');
      data = Object.fromEntries(parts.filter(part => !/filename=/.test(part.headers)).map(part => {
        const name = part.headers.match(/name="([^"]+)"/)?.[1];
        assert(name, 'multipart field must have a name');
        const text = part.value.toString('utf8');
        return [name, text.startsWith('"') ? JSON.parse(text) : text];
      }));
    } else {
      assert.match(String(req.headers['content-type']), /application\/json/);
      data = JSON.parse(body.toString('utf8'));
    }
    let cachedFileId;
    if (filePart) {
      this.#uploadCalls++;
      assert(filePart.value.length > 100 && filePart.value.length <= 2 * 1024 * 1024, 'uploaded media must stay within fixture bounds');
      const contract = this.#expectedUpload;
      if (contract) assert.equal(method, contract.method, 'upload must use the scenario expected Telegram method');
      const filename = filePart.headers.match(/filename="([^"]+)"/)?.[1];
      const mimeType = filePart.headers.match(/content-type:\s*([^\r\n]+)/i)?.[1]?.trim();
      uploadedMimeType = mimeType ?? contract?.mimeType;
      assert.equal(filename, contract?.filename ?? 'media.mp4', 'upload filename must match its default or explicit transform contract');
      // Teloxide streams file parts without a Content-Type header. If supplied,
      // validate it. Real scenario checks independently probe captured media bytes.
      if (mimeType !== undefined) assert.equal(mimeType, contract?.mimeType ?? 'video/mp4', 'upload MIME type must match its default or explicit transform contract');
      if (contract?.preserveMdat ?? true) {
        const sourceMdat = isoBmffMdatPayloads(this.mediaBytes);
        const uploadedMdat = isoBmffMdatPayloads(filePart.value);
        assert(sourceMdat.length > 0 && uploadedMdat.length > 0, 'source and upload must contain mdat boxes');
        assert.deepEqual(uploadedMdat, sourceMdat, 'remuxed media must preserve source mdat bytes');
      }
      this.#lastUploadedBytes = Buffer.from(filePart.value);
    } else {
      cachedFileId = data[method === 'sendvideo' ? 'video' : method === 'sendaudio' ? 'audio' : 'document'];
      assert(typeof cachedFileId === 'string' && this.#fileIds.has(cachedFileId), 'cache must reuse a previously confirmed file ID');
      this.#cachedCalls++;
    }
    const chat = Number(data.chat_id);
    assert(Number.isSafeInteger(chat), 'media payload must carry chat_id');
    const threadId = data.message_thread_id === undefined ? undefined : Number(data.message_thread_id);
    this.#calls.push({ method, status: this.#sendMode, cached: !!cachedFileId, uploadLength: filePart?.value.length ?? 0, receivedAt: performance.now() });
    if (this.#sendMode === 'invalid-file-id-once') {
      assert(cachedFileId, 'invalid-ID fixture must target a cached send');
      this.#sendMode = 'success';
      res.statusCode = 400;
      res.end(JSON.stringify({ ok: false, error_code: 400, description: 'Bad Request: wrong file identifier/HTTP URL specified' }));
      return;
    }
    if (this.#sendMode === 'hold') {
      await this.#sendGate;
      res.destroy();
      return;
    }
    if (this.#sendMode === 'socket-drop') { res.destroy(); return; }
    if (this.#sendMode === 'api-ambiguous') {
      res.statusCode = 500;
      res.end(JSON.stringify({ ok: false, error_code: 500, description: 'Internal Server Error' }));
      return;
    }
    if (this.#sendMode === 'rate-limit-once') {
      this.#sendMode = 'success';
      res.statusCode = 429;
      res.end(JSON.stringify({ ok: false, error_code: 429, description: 'Too Many Requests: retry after 2', parameters: { retry_after: 2 } }));
      return;
    }
    if (this.#sendMode === 'api-error') {
      res.statusCode = 400;
      res.end(JSON.stringify({ ok: false, error_code: 400, description: 'Bad Request: chat not found' }));
      return;
    }
    const kind = method === 'sendvideo' ? 'video' : method === 'sendaudio' ? 'audio' : 'document';
    const fileId = cachedFileId ?? `fixture-${kind}-${this.#sendCalls}`;
    this.#fileIds.add(fileId);
    const fileSize = filePart?.value.length ?? this.mediaBytes.length;
    const attachment = kind === 'video'
      ? { kind, value: { file_id: fileId, file_unique_id: `fixture-video-unique-${this.#sendCalls}`, width: 160, height: 120, duration: 1, mime_type: 'video/mp4', file_size: fileSize } }
      : kind === 'audio'
        ? { kind, value: { file_id: fileId, file_unique_id: `fixture-audio-unique-${this.#sendCalls}`, duration: 1, performer: 'Fixture', title: 'Fixture audio', mime_type: uploadedMimeType ?? 'audio/mpeg', file_size: fileSize } }
        : { kind, value: { file_id: fileId, file_unique_id: `fixture-document-unique-${this.#sendCalls}`, file_name: 'media.mp4', mime_type: 'video/mp4', file_size: fileSize } };
    res.end(JSON.stringify({ ok: true, result: this.#message(123456, chat, null, 9000 + this.#sendCalls, attachment, threadId) }));
  }
  #message(actor, chat, text, id = 1000, attachment, threadId) {
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
}
