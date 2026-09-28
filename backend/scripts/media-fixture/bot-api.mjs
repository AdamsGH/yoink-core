import assert from 'node:assert/strict';
import http from 'node:http';
import { isoBmffMdatPayloads } from './media-format.mjs';
import { parseMediaBody } from './multipart.mjs';

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
  #lastGroupBytes = [];
  #expectedAlbum;
  #sendPlan = [];
  #messages = new Map();
  #editMode = 'success';
  #editFilter;

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
  get lastGroupBytes() { return this.#lastGroupBytes; }
  setExpectedAlbum(items) {
    assert(items === null || (Array.isArray(items) && items.length >= 2 && items.length <= 10));
    this.#expectedAlbum = items;
  }
  setSendPlan(modes) { this.#sendPlan = [...modes]; }
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
  setEditMode(mode, textIncludes) { this.#editMode = mode; this.#editFilter = textIncludes; }
  get progressMessages() { return [...this.#messages.values()]; }
  holdNextUpload() {
    let release;
    this.#sendGate = new Promise(resolve => { release = resolve; });
    this.#sendMode = 'hold';
    return () => { release(); this.#sendGate = undefined; };
  }
  enqueue(actor, chat, text, id, language = 'en', { thread, senderChat, isBot = false } = {}) {
    const message = this.#message(actor, chat, text, id, undefined, thread);
    message.from.is_bot = isBot;
    if (senderChat) message.sender_chat = senderChat;
    message.from.language_code = language;
    this.#updates.push({ update_id: this.#updateId++, message });
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
      const message = this.#message(123456, Number(data.chat_id), data.text, ++this.#nextMessageId, undefined, data.message_thread_id);
      this.#messages.set(`${message.chat.id}:${message.message_id}`, message);
      this.#calls.push({ method, data, messageId: message.message_id });
      res.end(JSON.stringify({ ok: true, result: message }));
    } else if (method === 'editmessagetext') {
      const data = JSON.parse(body.toString('utf8'));
      assert(typeof data.text === 'string' && Buffer.byteLength(data.text) <= 4096, 'bounded progress text');
      const message = this.#messages.get(`${data.chat_id}:${data.message_id}`);
      const mode = this.#editFilter && !data.text.includes(this.#editFilter) ? 'success' : this.#editMode;
      if (mode.endsWith('-once')) this.#editMode = 'success';
      this.#calls.push({ method, data, status: mode, receivedAt: performance.now() });
      const fail = (code, description, parameters) => { res.statusCode = code; res.end(JSON.stringify({ ok: false, error_code: code, description, ...(parameters ? { parameters } : {}) })); };
      if (mode === 'rate-limit-once') { fail(429, 'Too Many Requests: retry after 2', { retry_after: 2 }); return; }
      if (mode === 'api-ambiguous-once') { fail(500, 'Internal Server Error'); return; }
      if (!message || mode === 'deleted') { fail(400, 'Bad Request: message to edit not found'); return; }
      if (message.text === data.text) { fail(400, 'Bad Request: message is not modified: specified new message content and reply markup are exactly the same as a current content and reply markup of the message'); return; }
      message.text = data.text;
      if (mode === 'socket-drop-once') { res.destroy(); return; }
      res.end(JSON.stringify({ ok: true, result: message }));
    } else if (method === 'sendmediagroup') {
      await this.#handleGroup(req, res, body);
    } else if (['sendvideo', 'senddocument', 'sendaudio', 'sendphoto'].includes(method)) {
      await this.#handleMedia(method, req, res, body);
    } else {
      res.writeHead(404);
      res.end(JSON.stringify({ ok: false, error_code: 404, description: 'unexpected fixture API method' }));
    }
  }
  async #handleMedia(method, req, res, body) {
    this.#sendCalls++;
    if (this.#sendPlan.length) this.#sendMode = this.#sendPlan.shift();
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
      cachedFileId = data[method === 'sendvideo' ? 'video' : method === 'sendaudio' ? 'audio' : method === 'sendphoto' ? 'photo' : 'document'];
      assert(typeof cachedFileId === 'string' && this.#fileIds.has(cachedFileId), 'cache must reuse a previously confirmed file ID');
      this.#cachedCalls++;
    }
    const chat = Number(data.chat_id);
    assert(Number.isSafeInteger(chat), 'media payload must carry chat_id');
    const threadId = data.message_thread_id === undefined ? undefined : Number(data.message_thread_id);
    const reply = typeof data.reply_parameters === 'string' ? JSON.parse(data.reply_parameters) : data.reply_parameters;
    this.#calls.push({ method, status: this.#sendMode, cached: !!cachedFileId, uploadLength: filePart?.value.length ?? 0, chat_id: chat, message_thread_id: threadId, reply_to_message_id: reply?.message_id, receivedAt: performance.now() });
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
    const kind = method === 'sendvideo' ? 'video' : method === 'sendaudio' ? 'audio' : method === 'sendphoto' ? 'photo' : 'document';
    const fileId = cachedFileId ?? `fixture-${kind}-${this.#sendCalls}`;
    this.#fileIds.add(fileId);
    const fileSize = filePart?.value.length ?? this.mediaBytes.length;
    const attachment = kind === 'photo'
      ? { kind, value: [{ file_id: fileId, file_unique_id: `fixture-photo-unique-${this.#sendCalls}`, width: 160, height: 120, file_size: fileSize }] }
      : kind === 'video'
      ? { kind, value: { file_id: fileId, file_unique_id: `fixture-video-unique-${this.#sendCalls}`, width: 160, height: 120, duration: 1, mime_type: 'video/mp4', file_size: fileSize } }
      : kind === 'audio'
        ? { kind, value: { file_id: fileId, file_unique_id: `fixture-audio-unique-${this.#sendCalls}`, duration: 1, performer: 'Fixture', title: 'Fixture audio', mime_type: uploadedMimeType ?? 'audio/mpeg', file_size: fileSize } }
        : { kind, value: { file_id: fileId, file_unique_id: `fixture-document-unique-${this.#sendCalls}`, file_name: 'media.mp4', mime_type: 'video/mp4', file_size: fileSize } };
    res.end(JSON.stringify({ ok: true, result: this.#message(123456, chat, null, 9000 + this.#sendCalls, attachment, this.#sendMode === 'wrong-thread' ? (threadId ?? 0) + 1 : threadId) }));
  }
  async #handleGroup(req, res, body) {
    this.#sendCalls++;
    if (this.#sendPlan.length) this.#sendMode = this.#sendPlan.shift();
    const { data, files } = parseMediaBody(String(req.headers['content-type'] ?? ''), body);
    const media = typeof data.media === 'string' ? JSON.parse(data.media) : data.media;
    assert(Array.isArray(media) && media.length >= 2 && media.length <= 10, 'album must contain 2..10 items');
    assert.equal(media.length, this.#expectedAlbum?.length, 'album must match an explicit item contract');
    const chat = Number(data.chat_id);
    assert(Number.isSafeInteger(chat), 'album must carry chat_id');
    const threadId = data.message_thread_id === undefined ? undefined : Number(data.message_thread_id);
    const used = new Set();
    const cached = files.size === 0;
    this.#lastGroupBytes = [];
    const attachments = media.map((item, index) => {
      const expected = this.#expectedAlbum[index];
      assert.equal(item.type, expected.kind, 'album item type/order');
      let bytes;
      let fileId;
      if (cached) {
        assert(this.#fileIds.has(item.media), 'album cache must use confirmed file IDs');
        fileId = item.media;
      } else {
        assert.match(item.media, /^attach:\/\//, 'album upload must reference an attachment');
        const name = item.media.slice('attach://'.length);
        assert(!used.has(name), 'album must not reuse multipart attachments');
        used.add(name);
        const part = files.get(name);
        assert(part, 'album attachment reference must resolve');
        bytes = part.value;
        assert(bytes.length > 100 && bytes.length <= 2 * 1024 * 1024, 'album item byte bounds');
        assert.equal(part.headers.match(/filename="([^"]+)"/)?.[1], expected.filename, 'album filename');
        const mimeType = part.headers.match(/content-type:\s*([^\r\n]+)/i)?.[1]?.trim();
        if (mimeType !== undefined) assert.equal(mimeType, expected.mimeType, 'album MIME type');
        if (expected.bytes) {
          if (expected.preserveMdat) {
            const actual = isoBmffMdatPayloads(bytes);
            const wanted = isoBmffMdatPayloads(expected.bytes);
            assert(actual.length > 0 && actual.length === wanted.length && actual.every((part, i) => part.equals(wanted[i])), 'album media content/order');
          } else assert(bytes.equals(expected.bytes), 'album media content/order');
        }
        this.#lastGroupBytes.push(Buffer.from(bytes));
        fileId = `fixture-${item.type}-${this.#sendCalls}-${index}`;
      }
      const file = { file_id: fileId, file_unique_id: `fixture-unique-${this.#sendCalls}-${index}`, file_size: bytes?.length ?? 128 };
      const value = item.type === 'photo' ? [{ ...file, width: 160, height: 120 }]
        : item.type === 'video' ? { ...file, width: 160, height: 120, duration: 1, mime_type: expected.mimeType }
          : item.type === 'audio' ? { ...file, duration: 1, mime_type: expected.mimeType }
            : { ...file, file_name: expected.filename, mime_type: expected.mimeType };
      return { kind: item.type, value, fileId };
    });
    assert.equal(used.size, files.size, 'album must not contain unreferenced files');
    if (cached) this.#cachedCalls++; else this.#uploadCalls++;
    this.#calls.push({ method: 'sendmediagroup', status: this.#sendMode, cached, uploadLength: this.#lastGroupBytes.reduce((n, b) => n + b.length, 0), itemCount: media.length, chat_id: chat, message_thread_id: threadId, receivedAt: performance.now() });
    if (this.#sendMode === 'socket-drop') { res.destroy(); return; }
    if (['api-error', 'api-ambiguous', 'rate-limit-once', 'invalid-file-id-once'].includes(this.#sendMode)) {
      const mode = this.#sendMode;
      if (mode === 'invalid-file-id-once') assert(cached, 'invalid file ID must target cache');
      if (mode.endsWith('-once')) this.#sendMode = 'success';
      const status = mode === 'api-ambiguous' ? 500 : mode === 'rate-limit-once' ? 429 : 400;
      res.statusCode = status;
      res.end(JSON.stringify({ ok: false, error_code: status, description: mode === 'invalid-file-id-once'
        ? 'Bad Request: wrong file identifier/HTTP URL specified'
        : mode === 'rate-limit-once' ? 'Too Many Requests: retry after 2'
          : mode === 'api-error' ? 'Bad Request: chat not found' : 'Internal Server Error',
      ...(status === 429 ? { parameters: { retry_after: 2 } } : {}) }));
      return;
    }
    for (const attachment of attachments) this.#fileIds.add(attachment.fileId);
    const messages = attachments.map(attachment => this.#message(123456, chat, null, ++this.#nextMessageId, attachment, threadId));
    if (this.#sendMode === 'short-response') messages.pop();
    if (this.#sendMode === 'duplicate-response') messages[1].message_id = messages[0].message_id;
    res.end(JSON.stringify({ ok: true, result: messages }));
  }
  #message(actor, chat, text, id = 1000, attachment, threadId) {
    const type = chat > 0 ? 'private' : threadId === undefined ? 'group' : 'supergroup';
    return {
      message_id: id, date: 1700000000,
      from: { id: actor, is_bot: false, first_name: 'Fixture', language_code: 'en' },
      chat: type === 'private' ? { id: chat, type } : { id: chat, type, title: 'Fixture group' },
      ...(text === null ? {} : { text }),
      ...(threadId === undefined ? {} : { message_thread_id: threadId, is_topic_message: true }),
      ...(attachment ? { [attachment.kind]: attachment.value } : {}),
    };
  
}
}
