// Host-only deterministic tests: uses the existing frontend TypeScript dependency,
// transpiles owned modules in memory, and mocks network/hooks (no generated output).
import assert from 'node:assert/strict'
import { test } from 'node:test'
import fs from 'node:fs'
import vm from 'node:vm'
import { createRequire } from 'node:module'
const require = createRequire(new URL('../../../../frontend/package.json', import.meta.url))
const ts = require('typescript')
const root = new URL('../src/', import.meta.url)
function load(path, mocks = {}) {
  const source = fs.readFileSync(new URL(path, root), 'utf8').replaceAll('import.meta.env.DEV', 'false')
  const js = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText
  const module = { exports: {} }
  vm.runInNewContext(js, { module, exports: module.exports, require: (name) => {
    if (!(name in mocks)) throw new Error(`Unexpected import ${name}`)
    return mocks[name]
  }, console, setTimeout, clearTimeout, ...mocks.globals }, { filename: path })
  return module.exports
}
function hooks() {
  const slots = []
  let index = 0
  let effects = []
  const same = (a, b) => a && b && a.length === b.length && a.every((x, i) => Object.is(x, b[i]))
  const react = {
    useState(initial) {
      const i = index++
      if (!slots[i]) slots[i] = { value: typeof initial === 'function' ? initial() : initial }
      return [slots[i].value, (next) => { slots[i].value = typeof next === 'function' ? next(slots[i].value) : next }]
    },
    useRef(value) { const i = index++; return slots[i] ??= { current: value } },
    useMemo(fn, deps) { const i = index++; if (!same(slots[i]?.deps, deps)) slots[i] = { value: fn(), deps }; return slots[i].value },
    useCallback(fn, deps) { return react.useMemo(() => fn, deps) },
    useEffect(fn, deps) {
      const i = index++
      if (!same(slots[i]?.deps, deps)) {
        effects.push(() => { slots[i]?.cleanup?.(); slots[i] = { deps, cleanup: fn() } })
      }
    },
  }
  return { react, render(fn) { index = 0; const result = fn(); const pending = effects; effects = []; pending.forEach((f) => f()); return result } }
}
const tick = async () => { for (let i = 0; i < 15; i++) await Promise.resolve() }
const deferred = () => { let resolve; const promise = new Promise((r) => { resolve = r }); return { promise, resolve } }
const page = (ids, next_cursor = null) => ({ items: ids.map((id) => ({ id, gh_repo_id: id + 1000 })), next_cursor })
const pagination = load('api/star-pagination.ts')
const operation = load('api/create-operation.ts')
const mutationsHelpers = load('api/mutations.ts', { '@core/lib/api-client': { apiClient: {} } })

function starsHarness() {
  const h = hooks(), requests = [], unstar = [], timers = new Map()
  let timerId = 0
  const api = {
    listFolders: async () => [], listStarLanguages: async () => [],
    listGhStars: (filters) => { const d = deferred(); requests.push({ filters, ...d }); return d.promise },
    unstarRepo: async (id) => { unstar.push(id) },
  }
  const hook = load('pages/useStarsPage.ts', {
    react: h.react, sonner: { toast: { error() {}, success() {} } },
    globals: { setTimeout: (fn) => { timers.set(++timerId, fn); return timerId }, clearTimeout: (id) => timers.delete(id) },
    '@inbox/api/items': api, '../api/star-pagination': pagination,
    './useMutationRecovery': { useMutationRecovery: () => ({ refresh() {} }) },
  }).useStarsPage
  return { render: () => h.render(hook), requests, unstar, debounce: () => { const tasks = [...timers.values()]; timers.clear(); tasks.forEach((fn) => fn()) } }
}

test('unstar targets local ID even when another row local ID collides with GitHub ID', async () => {
  const h = starsHarness(); h.render()
  h.requests[0].resolve({ items: [{ id: 7, gh_repo_id: 42 }, { id: 42, gh_repo_id: 7 }], next_cursor: null })
  await tick(); const view = h.render(); await view.onUnstar(7)
  assert.deepEqual(h.unstar, [7]); assert.deepEqual(Array.from(h.render().stars, (s) => s.id), [42])
})

for (const sort of ['starred_at', 'name']) test(`loadMore retains all filters for ${sort} and discards an old-folder response`, async () => {
  const h = starsHarness(); let view = h.render()
  view.setSelectedFolder(9); view.setLanguage('Rust'); view.setSort(sort); view.setSearch('query'); h.render(); h.debounce(); view = h.render()
  const initial = h.requests.at(-1); initial.resolve(page([1], sort === 'name' ? '30' : 'cursor-1'))
  await tick(); view = h.render(); const more = view.loadMore()
  const request = h.requests.at(-1)
  assert.equal(request.filters.search, 'query'); assert.equal(request.filters.folder_id, 9); assert.equal(request.filters.language, 'Rust'); assert.equal(request.filters.sort, sort)
  assert.equal(request.filters.limit, 30)
  assert.equal(sort === 'name' ? request.filters.offset : request.filters.cursor, sort === 'name' ? 30 : 'cursor-1')
  view.setSelectedFolder('unorganised'); h.render()
  const newInitial = h.requests.at(-1); assert.equal(newInitial.filters.folder_id, 0)
  newInitial.resolve(page([99])); await tick()
  request.resolve(page([2], 'next')); await more
  view = h.render(); assert.deepEqual(Array.from(view.stars, (s) => s.id), [99]); assert.equal(view.moreCursor, null)
  // The original initial request may arrive last, too.
  h.requests[0].resolve(page([999], 'stale')); await tick()
  assert.deepEqual(Array.from(h.render().stars, (s) => s.id), [99])
})

test('bounded requests continue past 200 members and candidates; cursor and offset retain filters', async () => {
  for (const sort of ['starred_at', 'name']) {
    const queries = []
    const stars = await pagination.collectStars(async (query) => {
      queries.push(query)
      const i = queries.length - 1
      return page(Array.from({ length: i < 2 ? 100 : 17 }, (_, n) => i * 100 + n), i < 2 ? String((i + 1) * 100) : null)
    }, { folder_id: 4, language: 'Rust', search: 'ab', sort })
    assert.equal(stars.length, 217); assert.equal(queries.length, 3)
    assert(queries.every((q) => q.limit === 100 && q.folder_id === 4 && q.language === 'Rust' && q.search === 'ab' && q.sort === sort))
    assert.equal(sort === 'name' ? queries[2].offset : queries[2].cursor, sort === 'name' ? 200 : '200')
  }
})

test('repeating continuation fails rather than presenting a truncated catalogue', async () => {
  await assert.rejects(pagination.collectStars(async () => page([1], 'same')), /Repeated/)
})

function storage() { const values = new Map(); return {
  getItem: (key) => values.get(key) ?? null,
  setItem: (key, value) => values.set(key, value),
  removeItem: (key) => values.delete(key),
} }
function recoveryHarness(store, api, mutationApi = {}, globals = {}) {
  const h = hooks(), notices = []
  const lockQueues = store.lockQueues ??= new Map()
  const locks = { request: (key, fn) => {
    const result = (lockQueues.get(key) ?? Promise.resolve()).then(fn)
    lockQueues.set(key, result.catch(() => {}))
    return result
  } }
  const hook = load('pages/useMutationRecovery.ts', {
    react: h.react,
    '@refinedev/core': { useGetIdentity: () => ({ data: { id: 'user-1' } }) },
    'react-i18next': { useTranslation: () => ({ t: (key) => key }) },
    sonner: { toast: { success: (x) => notices.push(x), error: (x) => notices.push(x) } },
    '../api/items': { listFolders: async () => [], ...api },
    '../api/create-operation': operation,
    '../api/mutations': { ...mutationsHelpers, listMutations: async () => [], ...mutationApi },
    globals: { localStorage: store, navigator: { locks }, crypto: { randomUUID: () => `key-${++recoveryHarness.serial}` }, ...globals },
  }).useMutationRecovery
  const onFolders = () => {}
  return { render: () => h.render(() => hook(onFolders)), notices }
}
recoveryHarness.serial = 0

test('lost response persists exact per-user identity across reload; explicit retry does not create twice; subsequent identical create is distinct', async () => {
  const store = storage(), requests = [], backend = new Map()
  let lose = true
  const createFolder = async (body, key) => {
    requests.push({ body, key })
    assert.equal(operation.readOperation(store, 'user-1').key, key) // durable before send
    if (!backend.has(key)) backend.set(key, { id: backend.size + 1, gh_list_id: `remote-${key}` })
    if (lose) { lose = false; throw new Error('response lost') }
    return backend.get(key)
  }
  let h = recoveryHarness(store, { createFolder }); let view = h.render(); await tick()
  const body = { name: 'identical', is_local: false }
  assert.equal(await view.create(body), null)
  assert.equal(backend.size, 1)
  assert.equal(operation.readOperation(store, 'other-user'), null)
  h = recoveryHarness(store, { createFolder }); view = h.render(); await tick(); view = h.render()
  assert.equal(requests.length, 1) // reload never automatically sends
  await view.retry(); assert.equal(requests.length, 2); assert.equal(requests[0].key, requests[1].key)
  assert.equal(JSON.stringify(requests[0].body), JSON.stringify(requests[1].body)); assert.equal(backend.size, 1)
  assert.equal(operation.readOperation(store, 'user-1'), null)
  view = h.render(); await view.create(body); assert.equal(backend.size, 2); assert.notEqual(requests[2].key, requests[0].key)
})

test('pending unknown create exposes recovery without success, resend, automatic adoption or clearing the saved key', async () => {
  const store = storage(); let sends = 0
  const h = recoveryHarness(store, { createFolder: async () => { sends++; throw { response: { data: { mutation_id: 'intent-1' } } } } }, {
    listMutations: async () => [{ id: 'intent-1', state: 'unknown' }],
    mutationStatus: async () => ({ id: 'intent-1', state: 'unknown', can_inspect_candidates: true, can_cancel: false }),
  })
  let view = h.render(); await tick(); await view.create({ name: 'ambiguous', is_local: false }); view = h.render()
  assert.equal(operation.readOperation(store, 'user-1').mutationId, 'intent-1')
  assert.equal(view.mutations[0].state, 'unknown'); assert.equal(operation.isApplied(view.mutations[0].state), false)
  await view.retry(); await view.refresh(); assert.equal(sends, 1)
  assert(!h.notices.includes('inboxRecovery.applied'))
  assert.equal(await view.create({ name: 'ambiguous', is_local: false }), null); assert.equal(sends, 1)
})

test('missing native recovery endpoints and unavailable durable storage fail closed before any create', async () => {
  for (const brokenStorage of [false, true]) {
    const store = storage(); let sends = 0
    if (brokenStorage) store.setItem = () => { throw new Error('storage blocked') }
    const h = recoveryHarness(store, { createFolder: async () => { sends++; return { gh_list_id: 'unexpected' } } }, {
      listMutations: async () => { if (!brokenStorage) throw new Error('404 legacy route'); return [] },
    })
    const view = h.render(); await tick(); assert.equal(await view.create({ name: 'test', is_local: false }), null)
    assert.equal(sends, 0); assert(!h.notices.includes('inboxRecovery.applied'))
  }
})

test('an unrelated conflict reaching applied does not clear this create key or claim success; explicit keyed confirmation is required', async () => {
  const store = storage(); let sends = 0; let state = 'unknown'; const keys = []
  const h = recoveryHarness(store, { createFolder: async (_body, key) => {
    keys.push(key)
    if (++sends === 1) throw { response: { data: { mutation_id: 'other-intent' } } }
    return { id: 3, gh_list_id: 'confirmed-list' }
  } }, {
    listMutations: async () => state === 'unknown' ? [{ id: 'other-intent', state }] : [],
    mutationStatus: async () => ({ id: 'other-intent', state }),
  })
  let view = h.render(); await tick(); await view.create({ name: 'ours', is_local: false })
  state = 'applied'; view = h.render(); await view.refresh(); view = h.render()
  assert.equal(operation.readOperation(store, 'user-1').key, keys[0])
  assert(!h.notices.includes('inboxRecovery.applied'))
  assert.equal(sends, 1)
  await view.retry(); assert.equal(sends, 2); assert.equal(keys[0], keys[1])
  assert.equal(operation.readOperation(store, 'user-1'), null)
  assert(h.notices.includes('inboxRecovery.applied'))
})

test('confirmed cancellation permits explicit discard, not an automatic new operation', async () => {
  const store = storage(); const op = operation.newOperation({ name: 'cancelled', is_local: false }, () => 'cancel-key')
  op.mutationId = 'cancelled-intent'; operation.saveOperation(store, 'user-1', op)
  const h = recoveryHarness(store, {}, { mutationStatus: async (id, key) => ({ id, state: 'rejected', operation_key_matches: key === op.key }) })
  h.render(); await tick(); const view = h.render()
  assert.equal(operation.readOperation(store, 'user-1').key, 'cancel-key')
  await view.discardRejected(); assert.equal(operation.readOperation(store, 'user-1'), null)
})

test('typed mutation adapter uses only actual native routes and explicit adoption ID', async () => {
  const requests = []
  const apiClient = Object.fromEntries(['get', 'post'].map((method) => [method, async (url, body) => {
    requests.push({ method, url, body }); return { data: method === 'get' ? [] : { state: 'unknown' } }
  }]))
  const api = load('api/mutations.ts', { '@core/lib/api-client': { apiClient } })
  await api.listMutations(); await api.mutationStatus('id/1'); await api.reconcileMutation('id/1')
  await api.createCandidates('id/1'); await api.adoptCreate('id/1', 'chosen-remote-id'); await api.cancelMutation('id/1')
  assert.deepEqual(requests.map((r) => `${r.method} ${r.url}`), [
    'get /inbox/gh_mutations', 'get /inbox/gh_mutations/id%2F1',
    'post /inbox/gh_mutations/id%2F1/reconcile', 'get /inbox/gh_mutations/id%2F1/create-candidates',
    'post /inbox/gh_mutations/id%2F1/adopt-create', 'post /inbox/gh_mutations/id%2F1/cancel',
  ])
  assert.equal(requests[4].body.remote_id, 'chosen-remote-id')
})

test('saved identity cannot be overwritten by another tab or stale retry', async () => {
  const store = storage(); let sends = 0
  const api = { createFolder: async () => { sends++; throw new Error('lost') } }
  const first = recoveryHarness(store, api), second = recoveryHarness(store, api)
  let view = first.render(); second.render(); await tick()
  await view.create({ name: 'first-tab', is_local: false })
  const key = operation.readOperation(store, 'user-1').key
  await second.render().create({ name: 'second-tab', is_local: false })
  assert.equal(sends, 1); assert.equal(operation.readOperation(store, 'user-1').key, key)
  view = first.render()
  operation.saveOperation(store, 'user-1', operation.newOperation({ name: 'replacement', is_local: false }, () => 'replacement-key'))
  await view.retry()
  assert.equal(sends, 1); assert.equal(operation.readOperation(store, 'user-1').key, 'replacement-key')
})

test('folder adapter sends exact stable Idempotency-Key and rejects unkeyed remote creates', async () => {
  const requests = []
  const api = load('api/items.ts', {
    '@core/lib/api-client': { apiClient: { post: async (...args) => { requests.push(args); return { data: { id: 1 } } } } },
    './star-pagination': pagination,
  })
  const body = { name: 'test', is_local: false }
  await assert.rejects(api.createFolder(body), /durable operation key/)
  assert.equal(requests.length, 0)
  await api.createFolder(body, 'stable-key'); await api.createFolder(body, 'stable-key')
  assert.equal(requests[0][2].headers['Idempotency-Key'], 'stable-key')
  assert.equal(requests[1][2].headers['Idempotency-Key'], 'stable-key')
  assert.equal(requests[0][1], body)
})

test('simultaneous tabs serialize admission and never replace an unresolved operation identity', async () => {
  const store = storage(), response = deferred(), admitted = deferred(); let sends = 0
  const api = { createFolder: async () => { sends++; admitted.resolve(); await response.promise; throw new Error('lost response') } }
  const a = recoveryHarness(store, api), b = recoveryHarness(store, api)
  const firstView = a.render(), secondView = b.render(); await tick()
  const first = firstView.create({ name: 'first', is_local: false })
  await admitted.promise
  const second = secondView.create({ name: 'second', is_local: false })
  const key = operation.readOperation(store, 'user-1').key
  response.resolve(); await Promise.all([first, second])
  assert.equal(sends, 1); assert.equal(operation.readOperation(store, 'user-1').key, key)
})

test('missing browser Web Locks fail closed before saving or sending a remote create', async () => {
  const store = storage(); let sends = 0
  const h = recoveryHarness(store, { createFolder: async () => { sends++ } }, {}, { navigator: {} })
  const view = h.render(); await tick(); assert.equal(await view.create({ name: 'test', is_local: false }), null)
  assert.equal(sends, 0); assert.equal(operation.readOperation(store, 'user-1'), null)
})

test('unorganised continuation retains folder_id zero and blocks duplicate concurrent loadMore calls', async () => {
  const h = starsHarness(); let view = h.render(); view.setSelectedFolder('unorganised'); h.render()
  h.requests.at(-1).resolve(page([1], 'cursor')); await tick(); view = h.render()
  const count = h.requests.length; const first = view.loadMore(); await view.loadMore()
  assert.equal(h.requests.length, count + 1)
  assert.equal(h.requests.at(-1).filters.folder_id, 0)
  h.requests.at(-1).resolve(page([2])); await first
  assert.deepEqual(Array.from(h.render().stars, (s) => s.id), [1, 2])
})

test('unrelated conflict rejection or missing proof cannot discard the saved create or admit a new key', async () => {
  for (const proof of [false, undefined]) {
    const store = storage(), checks = []; let sends = 0
    const op = operation.newOperation({ name: 'uncertain', is_local: false }, () => 'uncertain-key')
    op.mutationId = 'unrelated-rejected'; operation.saveOperation(store, 'user-1', op)
    const h = recoveryHarness(store, { createFolder: async () => { sends++ } }, {
      listMutations: async () => [{ id: 'unrelated-rejected', state: 'rejected' }, { id: 'another-active', state: 'unknown' }],
      mutationStatus: async (id, key) => {
        checks.push({ id, key })
        return { id, state: id === op.mutationId ? 'rejected' : 'unknown', operation_key_matches: proof }
      },
    })
    h.render(); await tick(); const view = h.render()
    assert.equal(checks.find((c) => c.id === op.mutationId).key, op.key)
    assert.equal(checks.find((c) => c.id === 'another-active').key, undefined)
    await view.discardRejected()
    assert.equal(operation.readOperation(store, 'user-1').key, op.key)
    assert.equal(await view.create({ name: 'uncertain', is_local: false }), null)
    assert.equal(sends, 0)
  }
})

test('matching rejected key requires fresh proof under the lock before explicit discard', async () => {
  for (const freshProof of [true, false, undefined]) {
    const store = storage(), checks = []
    const op = operation.newOperation({ name: 'rejected', is_local: false }, () => 'matching-key')
    op.mutationId = 'matching-rejected'; operation.saveOperation(store, 'user-1', op)
    const h = recoveryHarness(store, {}, {
      mutationStatus: async (id, key) => {
        checks.push({ id, key })
        return { id, state: 'rejected', operation_key_matches: checks.length === 1 ? true : freshProof }
      },
    })
    h.render(); await tick(); const view = h.render()
    assert.equal(operation.readOperation(store, 'user-1').key, op.key) // refresh alone never discards
    await view.discardRejected()
    assert(checks.every((c) => c.id === op.mutationId && c.key === op.key))
    assert.equal(operation.readOperation(store, 'user-1')?.key ?? null, freshProof === true ? null : op.key)
  }
})

test('applied matching-key status retains identity until explicit keyed response retrieval', async () => {
  const store = storage(); let sends = 0
  const op = operation.newOperation({ name: 'applied', is_local: false }, () => 'applied-key')
  op.mutationId = 'applied-intent'; operation.saveOperation(store, 'user-1', op)
  const h = recoveryHarness(store, { createFolder: async (_body, key) => {
    sends++; assert.equal(key, op.key); return { id: 7, gh_list_id: 'confirmed-remote' }
  } }, { mutationStatus: async (id, key) => ({ id, state: 'applied', operation_key_matches: key === op.key }) })
  h.render(); await tick(); const view = h.render()
  await view.discardRejected(); assert.equal(operation.readOperation(store, 'user-1').key, op.key)
  assert.equal(sends, 0); await view.retry()
  assert.equal(sends, 1); assert.equal(operation.readOperation(store, 'user-1'), null)
})

test('status adapter supplies Idempotency-Key only when explicitly passed for the associated operation', async () => {
  const calls = []
  const api = load('api/mutations.ts', { '@core/lib/api-client': {
    apiClient: { get: async (url, config) => { calls.push({ url, config }); return { data: {} } } },
  } })
  await api.mutationStatus('associated-id', 'saved-key'); await api.mutationStatus('unrelated-id')
  assert.equal(calls[0].config.headers['Idempotency-Key'], 'saved-key')
  assert.equal(calls[1].config.headers, undefined)
})
