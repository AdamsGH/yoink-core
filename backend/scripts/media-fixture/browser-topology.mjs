import assert from 'node:assert/strict';
import { createHash, createPublicKey } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { get } from 'node:https';
import { setTimeout as sleep } from 'node:timers/promises';

export function renderBrowserProxy(template, { backendPort, frontendPort, proxyPort, httpsPort }) {
  for (const port of [backendPort, frontendPort, proxyPort, httpsPort]) {
    assert(Number.isInteger(port) && port > 1024 && port <= 65535, 'fixture requires unprivileged TCP ports');
  }
  assert.equal(new Set([backendPort, frontendPort, proxyPort, httpsPort]).size, 4, 'fixture ports must differ');
  for (const text of ['worker_processes auto;', 'listen 8081;', 'set $backend  yoink:8000;', 'set $frontend yoink-frontend:80;', '${DEV_ALLOWED_CIDR}']) {
    assert.equal(template.split(text).length, 2, 'project proxy template contract changed');
  }
  const configured = template
    .replace('worker_processes auto;', 'worker_processes 1;')
    .replace('listen 8081;', `listen 127.0.0.1:${proxyPort};`)
    .replace('set $backend  yoink:8000;', `set $backend  127.0.0.1:${backendPort};`)
    .replace('set $frontend yoink-frontend:80;', `set $frontend 127.0.0.1:${frontendPort};`)
    .replace('${DEV_ALLOWED_CIDR}', '127.0.0.1/32');
  const end = configured.lastIndexOf('}');
  assert(end >= 0 && configured.slice(end + 1).trim() === '', 'project proxy template must end with the HTTP block');
  // TLS termination belongs to this fixture. The project HTTP routing remains unchanged.
  return `${configured.slice(0, end)}
    server {
        listen 127.0.0.1:${httpsPort} ssl;
        ssl_certificate /fixture/browser-cert.pem;
        ssl_certificate_key /fixture/browser-key.pem;
        location / {
            proxy_pass http://127.0.0.1:${proxyPort};
            proxy_set_header Host $http_host;
            proxy_set_header X-Forwarded-Proto https;
        }
    }
${configured.slice(end)}`;
}

export async function startBrowserTopology({ workspace, tempRoot, runId, backendPort, freeLoopbackPort, capture, startContainer }) {
  const project = path.dirname(workspace.replace(/\/$/, ''));
  const directory = path.join(tempRoot, 'browser-proxy');
  await mkdir(directory);
  const frontendPort = await freeLoopbackPort();
  const proxyPort = await freeLoopbackPort();
  const httpsPort = await freeLoopbackPort();
  await capture('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=localhost', '-addext', 'subjectAltName=DNS:localhost,IP:127.0.0.1', '-keyout', path.join(directory, 'browser-key.pem'), '-out', path.join(directory, 'browser-cert.pem')], { timeout: 15000 });
  const certificate = await readFile(path.join(directory, 'browser-cert.pem'));
  const spki = createHash('sha256').update(createPublicKey(certificate).export({ type: 'spki', format: 'der' })).digest('base64');
  const template = await readFile(path.join(project, 'docker/nginx/nginx.conf.template'), 'utf8');
  await writeFile(path.join(directory, 'nginx.conf'), renderBrowserProxy(template, { backendPort, frontendPort, proxyPort, httpsPort }));
  const limits = ['--cpus', '1', '--memory', '128m', '--pids-limit', '64'];
  await startContainer(['--name', `yoink-browser-frontend-${runId}`, ...limits, '--env', 'NGINX_ENTRYPOINT_WORKER_PROCESSES_AUTOTUNE=1', '-p', `127.0.0.1:${frontendPort}:80`, 'yoink-fixture-frontend:browser']);
  await startContainer(['--name', `yoink-browser-proxy-${runId}`, ...limits, '--network', 'host', '--mount', `type=bind,source=${directory},target=/fixture,readonly`, '--entrypoint', 'nginx', 'nginx:alpine', '-c', '/fixture/nginx.conf', '-g', 'daemon off;']);
  const origin = `https://127.0.0.1:${httpsPort}`;
  let ready = false;
  for (let attempt = 0; attempt < 25; attempt++) {
    ready = await new Promise(resolve => {
      const request = get(`${origin}/cookies`, { ca: certificate, agent: false, timeout: 1000 }, response => {
        const valid = response.statusCode === 200 && response.headers['content-type']?.includes('text/html');
        response.once('end', () => resolve(valid));
        response.once('error', () => resolve(false));
        response.resume();
      });
      request.once('timeout', () => request.destroy());
      request.once('error', () => resolve(false));
    });
    if (ready) break;
    await sleep(200);
  }
  assert(ready, 'fixture HTTPS proxy must serve the existing SPA deep link');
  return { origin, spki };
}
