import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { renderBrowserProxy } from './browser-topology.mjs';

const template = await readFile(new URL('../../../docker/nginx/nginx.conf.template', import.meta.url), 'utf8');
const ports = { backendPort: 18000, frontendPort: 18001, proxyPort: 18002, httpsPort: 18003 };

test('browser proxy preserves project routes and adds fixture TLS termination', () => {
  const configured = renderBrowserProxy(template, ports);
  const projectPrefix = configured.slice(0, configured.indexOf('    server {\n        listen 127.0.0.1:18003 ssl;'));
  assert.equal(projectPrefix, template.slice(0, template.lastIndexOf('}'))
    .replace('worker_processes auto;', 'worker_processes 1;')
    .replace('listen 8081;', 'listen 127.0.0.1:18002;')
    .replace('set $backend  yoink:8000;', 'set $backend  127.0.0.1:18000;')
    .replace('set $frontend yoink-frontend:80;', 'set $frontend 127.0.0.1:18001;')
    .replace('${DEV_ALLOWED_CIDR}', '127.0.0.1/32') + '\n');
  assert(configured.includes('proxy_set_header X-Forwarded-Proto https;'));
  assert(configured.includes('ssl_certificate /fixture/browser-cert.pem;'));
  assert(configured.includes('proxy_pass http://127.0.0.1:18002;'));
});

test('browser proxy rejects unsafe ports and changed production template', () => {
  for (const backendPort of [80, 65536, '18000', 18001]) {
    assert.throws(() => renderBrowserProxy(template, { ...ports, backendPort }));
  }
  assert.throws(() => renderBrowserProxy(template.replace('listen 8081;', 'listen 8082;'), ports), /template contract changed/);
  assert.throws(() => renderBrowserProxy(template + 'unexpected suffix', ports), /must end/);
});
