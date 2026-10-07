import assert from 'node:assert/strict';
import http from 'node:http';

// Only operator-selected endpoints use this service. User URL policy is unchanged.
export class TrustedServices {
  calls = [];
  lists = [];
  dropCreate = false;
  holdRepository = false;
  holdStars = false;
  deadlineStars = false;
  deadlineRequests = [];
  held = 0;
  #server;
  #sockets = new Set();
  async start() {
    this.#server = http.createServer((req, res) => this.#handle(req, res).catch(() => { res.statusCode = 400; res.end(); }));
    this.#server.on('connection', socket => { this.#sockets.add(socket); socket.once('close', () => this.#sockets.delete(socket)); });
    await new Promise(resolve => this.#server.listen(0, '127.0.0.1', resolve));
    this.origin = `http://127.0.0.1:${this.#server.address().port}`;
    return this;
  }
  async close() { for (const socket of this.#sockets) socket.destroy(); if (this.#server) await new Promise(resolve => this.#server.close(resolve)); }
  async #handle(req, res) {
    let bytes = 0, chunks = [];
    for await (const chunk of req) { bytes += chunk.length; assert(bytes <= 128 * 1024); chunks.push(chunk); }
    const body = bytes ? JSON.parse(Buffer.concat(chunks).toString()) : null;
    const url = new URL(req.url, 'http://fixture');
    this.calls.push({ method: req.method, path: url.pathname, body });
    const json = data => { res.setHeader('content-type', 'application/json'); res.end(JSON.stringify(data)); };
    if (url.pathname === '/v1/chat/completions') {
      assert.equal(req.method, 'POST'); assert(Array.isArray(body.messages));
      return json({ choices: [{ message: { role: 'assistant', content: JSON.stringify({ categories: [{ name: 'Fixture', kind: 'new', confidence: 1 }], summary: 'Native fixture classification.' }) }, finish_reason: 'stop' }] });
    }
    if (url.pathname.startsWith('/repos/fixture/')) {
      if (this.holdRepository) { this.held++; return; }
      if (url.pathname.endsWith('/readme')) return json({ content: Buffer.from('# Deterministic native Inbox README\nReal enrichment fixture.').toString('base64'), encoding: 'base64' });
      return json({ id: 991, node_id: 'REPO_991', full_name: url.pathname.slice(7), name: url.pathname.split('/').at(-1), description: 'Native fixture repository', html_url: `https://github.com/${url.pathname.slice(7)}`, language: 'Rust', default_branch: 'main', license: { spdx_id: 'MIT' }, stargazers_count: 1 });
    }
    if (url.pathname === '/user/starred' && req.method === 'GET' && this.deadlineStars) {
      const record = { page: url.searchParams.get('page') ?? '1', started: performance.now(), closedAfter: null };
      this.deadlineRequests.push(record);
      res.once('close', () => { record.closedAfter = performance.now() - record.started; });
      if (record.page === '2') return; // second bounded read spans the outer job deadline
      res.setHeader('link', `<${this.origin}/user/starred?page=2>; rel="next"`);
      const timer = setTimeout(() => json([]), 3000);
      res.once('close', () => clearTimeout(timer));
      return;
    }
    if (url.pathname === '/user/starred' && req.method === 'GET' && this.holdStars) return;
    if (url.pathname === '/user/starred' && req.method === 'GET') return json([{ starred_at: '2026-01-01T00:00:00Z', repo: { id: 991, node_id: 'REPO_991', name: 'repo', full_name: 'fixture/repo', html_url: 'https://github.com/fixture/repo', description: 'Fixture star', language: 'Rust', stargazers_count: 1, owner: { login: 'fixture' } } }]);
    if (url.pathname.startsWith('/user/starred/') && ['PUT', 'DELETE'].includes(req.method)) { res.statusCode = 204; return res.end(); }
    if (url.pathname === '/graphql') {
      const query = body.query;
      const pageInfo = { hasNextPage: false, endCursor: null };
      if (query.includes('createUserList')) {
        const list = { id: `LIST_${this.lists.length + 1}`, name: body.variables.input.name, slug: `list-${this.lists.length + 1}`, description: body.variables.input.description };
        this.lists.push(list);
        if (this.dropCreate) { this.dropCreate = false; res.destroy(); return; }
        return json({ data: { createUserList: { list } } });
      }
      if (query.includes('viewer')) return json({ data: { viewer: { login: 'fixture' } } });
      if (query.includes('user(login:')) return json({ data: { user: { lists: { nodes: this.lists, pageInfo } } } });
      if (query.includes('items(')) return json({ data: { node: { items: { nodes: [], pageInfo } } } });
    }
    res.statusCode = 404; json({ error: 'unexpected fixture route' });
  }
}

// Reuse the established Telegram protocol fake; lose *only* a real receipt.
export async function receiptProxy(target) {
  const state = { dropSend: false, dropEdit: false, calls: [] };
  const sockets = new Set();
  const server = http.createServer((req, res) => {
    const method = req.url.split('/').at(-1).toLowerCase();
    const drop = method === 'sendmessage' ? state.dropSend : method === 'editmessagetext' ? state.dropEdit : false;
    if (drop) { if (method === 'sendmessage') state.dropSend = false; else state.dropEdit = false; }
    state.calls.push({ method, drop });
    const upstream = http.request(target + req.url, { method: req.method, headers: req.headers }, reply => {
      // Forward the request, consume the actual Bot API response, then cut the receipt.
      if (drop) { reply.resume(); reply.once('end', () => res.destroy()); }
      else { res.writeHead(reply.statusCode, reply.headers); reply.pipe(res); }
    });
    upstream.once('error', () => res.destroy());
    req.pipe(upstream);
    res.once('close', () => upstream.destroy());
  });
  server.on('connection', socket => { sockets.add(socket); socket.once('close', () => sockets.delete(socket)); });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  state.origin = `http://127.0.0.1:${server.address().port}`;
  state.close = async () => { for (const socket of sockets) socket.destroy(); await new Promise(resolve => server.close(resolve)); };
  return state;
}
