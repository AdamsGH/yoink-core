import http from 'node:http';
import { pathToFileURL } from 'node:url';

export function socketPing(socket, timeout = 2000) {
  return new Promise((resolve, reject) => {
    const request = http.request({ socketPath: socket, path: '/health', method: 'GET', agent: false }, response => {
      response.resume();
      response.on('end', () => response.statusCode === 200 ? resolve() : reject(new Error('gateway unavailable')));
      response.on('error', reject);
    });
    request.setTimeout(timeout, () => request.destroy(new Error('gateway timeout')));
    request.on('error', reject);
    request.end();
  });
}

export async function healthy(env = process.env) {
  await Promise.all(['http://127.0.0.1:9173/health', 'http://127.0.0.1:4416/ping'].map(async url => {
    const response = await fetch(url, { signal: AbortSignal.timeout(2000) });
    if (!response.ok) throw new Error('YouTube provider unavailable');
    await response.body?.cancel();
  }));
  await socketPing(env.YOINK_YOUTUBE_AUTH_SOCKET);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  healthy().catch(() => { process.exitCode = 1; });
}
