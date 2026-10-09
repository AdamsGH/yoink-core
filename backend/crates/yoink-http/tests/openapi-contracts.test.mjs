import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = fileURLToPath(new URL('../../../../', import.meta.url));
const read = (path) => readFileSync(root + path, 'utf8');
const document = JSON.parse(read('backend/crates/yoink-http/src/configuration-openapi.json'));
test('Inbox remote idempotency, ownership proof and accepted rule runs match handlers', () => {
  const creation = document.paths['/api/v1/inbox/folders'].post;
  const status = document.paths['/api/v1/inbox/gh_mutations/{id}'].get;
  const key = operation => operation.parameters.find(parameter => parameter.in === 'header' && parameter.name === 'Idempotency-Key');
  assert.equal(key(creation).required, false);
  assert.match(key(creation).description, /Required when is_local is false/);
  assert.match(key(creation).description, /Reuse the same key and unchanged request/);
  assert.equal(key(status).required, false);
  assert.match(key(status).description, /operation_key_matches/);
  assert.equal(key(status).schema.maxLength, 128);
  const responses = document.paths['/api/v1/inbox/rules/{id}/run'].post.responses;
  assert.ok(responses['202']);
  assert.equal(responses['200'], undefined);
  assert.equal(responses['202'].content['application/json'].schema.$ref, '#/components/schemas/InboxRuleRun');
});
const sources = ['lib', 'health', 'auth', 'admin', 'groups', 'configuration', 'cookie_web',
  'downloader_settings', 'user_session', 'm2m', 'metrics', 'music', 'activity', 'stats_imports']
  .map((name) => ['backend/crates/yoink-http/src/' + name + '.rs', name === 'stats_imports' ? '/api/v1/stats' : ''])
  .concat(['inbox', 'insight', 'stats'].map((name) => ['backend/crates/yoink-' + name + '/src/http.rs', '/api/v1/' + name]));

// Route expressions may contain nested handlers and JSON. Strings and comments are not delimiters.
function closing(source, start, open = '(', close = ')') {
  let depth = 0;
  for (let i = start; i < source.length; i++) {
    if (source.slice(i, i + 2) === '//') {
      i = source.indexOf('\n', i);
      if (i < 0) break;
      continue;
    }
    if (source.slice(i, i + 2) === '/*') {
      i = source.indexOf('*/', i + 2) + 1;
      continue;
    }
    if (source[i] === '"') {
      for (i++; i < source.length; i++) {
        if (source[i] === '\\') i++;
        else if (source[i] === '"') break;
      }
      continue;
    }
    const character = source.slice(i).match(/^'(?:\\.|[^'\\])'/);
    if (character) { i += character[0].length - 1; continue; }
    if (source[i] === open) depth++;
    if (source[i] === close && --depth === 0) return i;
  }
  throw new Error('Unbalanced native route source');
}

function productionSource(path) {
  let source = read(path);
  const marker = /#\[cfg\(test\)\]\s*mod \w+\s*\{/g;
  for (let match; (match = marker.exec(source)); ) {
    const end = closing(source, match.index + match[0].length - 1, '{', '}');
    source = source.slice(0, match.index) + source.slice(end + 1);
    marker.lastIndex = match.index;
  }
  return source;
}

function sourceInventory() {
  const result = new Map();
  for (const [file, prefix] of sources) {
    const source = productionSource(file);
    for (const match of source.matchAll(/\.route\(\s*"([^"]+)"\s*,/g)) {
      const end = closing(source, match.index + 6);
      const methods = source.slice(match.index + match[0].length, end);
      for (const method of methods.matchAll(/\b(get|post|put|patch|delete)\(\s*(\w+|\|\|)/g)) {
        const key = method[1] + ' ' + prefix + match[1];
        assert.ok(!result.has(key), 'Duplicate registered operation: ' + key);
        result.set(key, { file, handler: method[2] === '||' ? 'closure' : method[2] });
      }
    }
  }
  return result;
}

function resolve(reference) {
  assert.ok(reference.startsWith('#/'), 'Only local schema references are expected');
  return reference.slice(2).split('/').map((part) => part.replaceAll('~1', '/').replaceAll('~0', '~'))
    .reduce((value, part) => value?.[part], document);
}
function schema(value) { return value.$ref ? resolve(value.$ref) : value; }
function response(path, method = 'get', code = '200') {
  const value = schema(document.paths[path][method].responses[code]);
  return schema(value.content['application/json'].schema);
}
function request(path, method, media = 'application/json') {
  return schema(document.paths[path][method].requestBody.content[media].schema);
}
function visit(value, path, callback) {
  if (!value || typeof value !== 'object') return;
  callback(value, path);
  for (const [key, child] of Object.entries(value)) visit(child, path + '/' + key, callback);
}

test('native route/method inventory equals the served document, including product mounts', () => {
  const inventory = sourceInventory();
  const documented = new Map();
  for (const [path, methods] of Object.entries(document.paths)) for (const [method, op] of Object.entries(methods)) {
    const key = method + ' ' + path;
    documented.set(key, { file: op['x-source'], handler: op['x-handler'] });
    assert.ok(op.operationId && op.summary && Object.keys(op.responses).some((code) => +code >= 200 && +code < 300), key);
    const parameters = op.parameters ?? [];
    for (const match of path.matchAll(/\{(\w+)\}/g)) {
      assert.ok(parameters.some((p) => p.name === match[1] && p.in === 'path' && p.required), key + ': missing path parameter');
    }
    assert.equal(new Set(parameters.map((p) => p.in + ':' + p.name)).size, parameters.length, key + ': duplicate parameters');
  }
  assert.deepEqual([...documented.keys()].sort(), [...inventory.keys()].sort());
  for (const [key, source] of inventory) assert.deepEqual(documented.get(key), source, key);
  assert.equal(new Set(Object.values(document.paths).flatMap((methods) => Object.values(methods).map((op) => op.operationId))).size, inventory.size);
  assert.ok(inventory.size > 150);
});

test('all local references and source pointers resolve; no placeholder success schemas', () => {
  assert.equal(document.openapi, '3.1.0');
  visit(document, '', (value, path) => {
    if (value.$ref) assert.ok(resolve(value.$ref), path + ': unresolved ' + value.$ref);
    if (value['x-source']) assert.ok(existsSync(root + value['x-source']), path + ': missing source');
    if (value.required && value.properties) for (const key of value.required) assert.ok(key in value.properties, path + ': invalid required key');
    if (value.items) assert.notDeepEqual(value.items, {}, path + ': untyped array item');
  });
  for (const [path, methods] of Object.entries(document.paths)) for (const [method, op] of Object.entries(methods)) {
    for (const [code, raw] of Object.entries(op.responses)) {
      const result = schema(raw);
      assert.ok(result.description, method + ' ' + path + ' ' + code);
      if (+code >= 200 && +code < 300 && result.content) assert.notDeepEqual(result.content['application/json']?.schema, {});
    }
    for (const security of op.security ?? document.security) for (const scheme of Object.keys(security)) {
      assert.ok(document.components.securitySchemes[scheme], 'Unknown security scheme ' + scheme);
    }
  }
});

test('Scalar bearer, API-key and body-token authentication match actual consumers', () => {
  assert.match(read('frontend/src/pages/admin/api-docs/index.tsx'), /preferredSecurityScheme:\s*'HTTPBearer'/);
  assert.deepEqual(document.security, [{ HTTPBearer: [] }]);
  assert.equal(document.components.securitySchemes.HTTPBearer.scheme, 'bearer');
  assert.equal(document.components.securitySchemes.ApiKey.name, 'X-Api-Key');
  assert.deepEqual(document.paths['/api/internal/v1/status'].get.security, [{ ApiKey: [] }]);
  assert.deepEqual(document.paths['/api/v1/dl/cookies/submit'].post.security, []);
  assert.ok(request('/api/v1/dl/cookies/submit', 'post').required.includes('token'));
  assert.match(document.paths['/api/v1/auth/dev'].post.description, /only.*development/);
  for (const path of ['/openapi.json', '/api/v1/openapi.json', '/docs', '/api/v1/stats/export', '/api/v1/stats/import/{job_id}/cancel']) assert.ok(!document.paths[path]);
});

test('configuration and thread contracts survive aggregation with real defaults and identifiers', () => {
  const bot = request('/api/v1/bot-settings', 'patch');
  assert.deepEqual(Object.keys(bot.properties).sort(), ['bot_access_mode', 'browser_cookies_min_role', 'inline_storage_chat_id', 'inline_storage_thread_id']);
  assert.equal(bot.additionalProperties, false);
  assert.ok(bot.properties.inline_storage_chat_id.anyOf.some((s) => s.type === 'string'));
  assert.ok(document.paths['/api/v1/bot-settings/tag-map'].put);
  assert.ok(document.paths['/api/v1/bot-settings/available-features'].get);
  assert.deepEqual(Object.keys(response('/api/v1/threads/status').properties), ['available']);
  assert.ok(document.paths['/api/v1/threads/scan/{chat}'].post.responses['200']);
  assert.equal(request('/api/v1/groups/{id}/threads', 'post').properties.enabled.default, true);
  assert.ok(!request('/api/v1/groups/{id}/threads', 'post').required.includes('enabled'));
});

test('Stats wildcard enum is derived from the actual native acceptance branches', () => {
  const filtered = read('backend/crates/yoink-stats/src/filtered.rs');
  const allow = filtered.slice(filtered.indexOf('if !matches!'), filtered.indexOf('return Err(Error::Invalid)', filtered.indexOf('if !matches!')));
  const accepted = [...allow.matchAll(/"([a-z-]+)"/g)].map((m) => m[1]);
  accepted.push('chat-admins', 'member-events', 'top-reactions');
  const op = document.paths['/api/v1/stats/{endpoint}'].get;
  assert.deepEqual(op.parameters.find((p) => p.name === 'endpoint').schema.enum, accepted.sort());
  const results = op.responses['200'].content['application/json'].schema.anyOf.map(schema);
  assert.deepEqual(results.map((s) => s['x-endpoint']).sort(), accepted.sort());
  assert.equal(response('/api/v1/stats/members/sync', 'post').type, 'array');
});

test('Music, Stats imports and metrics document actual response and intake shapes', () => {
  assert.deepEqual(Object.keys(response('/api/v1/music/me/stats').properties).sort(), ['by_day', 'today', 'top_artists', 'top_platforms', 'total', 'this_week'].sort());
  const metrics = response('/metrics');
  assert.deepEqual(Object.keys(metrics.properties).sort(), ['counters', 'histograms', 'uptime_seconds']);
  assert.equal(Object.keys(metrics.properties.counters.properties).length, 8);
  assert.equal(Object.keys(metrics.properties.histograms.properties).length, 2);
  const imported = response('/api/v1/stats/import', 'post');
  assert.deepEqual(imported.properties.status.enum, ['running', 'done', 'error']);
  assert.deepEqual(response('/api/v1/stats/import/{job_id}').properties, imported.properties);
  assert.equal(request('/api/v1/stats/import', 'post', 'multipart/form-data').properties.file.format, 'binary');
  assert.deepEqual(document.paths['/api/v1/stats/import'].post.parameters.map((p) => [p.name, p.in, p.required]), [['chat_id', 'query', true]]);
  assert.deepEqual(Object.keys(request('/api/v1/stats/import/by-path', 'post').properties).sort(), ['chat_id', 'path']);
});

test('typed authentication and settings fields agree with source declarations', () => {
  for (const [file, name, path, method] of [
    ['backend/crates/yoink-http/src/auth.rs', 'TokenRequest', '/api/v1/auth/token', 'post'],
    ['backend/crates/yoink-http/src/auth.rs', 'SettingsRequest', '/api/v1/settings', 'patch'],
    ['backend/crates/yoink-downloads/src/user_settings.rs', 'UserSettingsPatch', '/api/v1/dl/settings', 'patch'],
    ['backend/crates/yoink-insight/src/management.rs', 'ByokUpdate', '/api/v1/insight/byok/me', 'put'],
  ]) {
    const source = read(file), start = source.indexOf('struct ' + name + ' {');
    assert.ok(start >= 0);
    const body = source.slice(source.indexOf('{', start) + 1, closing(source, source.indexOf('{', start), '{', '}'));
    const fields = [...body.matchAll(/^\s*(?:pub\s+)?(\w+):/gm)].map((m) => m[1]);
    assert.deepEqual(Object.keys(request(path, method).properties).sort(), fields.sort(), name);
  }
});
