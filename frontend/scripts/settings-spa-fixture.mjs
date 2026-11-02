// Real SPA and native settings API on disposable state, with no Telegram or provider I/O.
import assert from 'node:assert/strict'
import { execFile, spawn } from 'node:child_process'
import { createHmac } from 'node:crypto'
import { once } from 'node:events'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'
import { createServer } from 'vite'

const root = fileURLToPath(new URL('../', import.meta.url))
const binary = fileURLToPath(new URL('../../backend/target/debug/yoink-server', import.meta.url))
const directory = await mkdtemp(path.join(tmpdir(), 'yoink-settings-spa-'))
const bot = '123456:settings-fixture'
const env = {
  PATH: process.env.PATH,
  TMPDIR: directory, TURSO_TMPDIR: directory,
  YOINK_RUST_DATABASE_PATH: path.join(directory, 'settings.db'),
  YOINK_RUST_HTTP_BIND: '127.0.0.1:0', YOINK_RUST_TELEGRAM_ENABLED: 'false',
  YOINK_RUST_AUTH_ENABLED: 'true', YOINK_RUST_OWNER_ID: '42',
  YOINK_RUST_TELEGRAM_TOKEN: bot,
  YOINK_RUST_API_SECRET_KEY: 'settings-fixture-secret-at-least-32bytes',
}
let child, exited, vite
let stopping = false
const stopped = new Promise(resolve => {
  for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, () => { stopping = true; resolve() })
})
try {
  await promisify(execFile)(binary, ['migrate'], { env, timeout: 15000 })
  child = spawn(binary, [], { env, stdio: ['ignore', 'pipe', 'pipe'] })
  exited = once(child, 'exit')
  let output = ''
  for (const stream of [child.stdout, child.stderr]) stream.on('data', chunk => { output = (output + chunk).slice(-16384) })
  let address
  for (let attempt = 0; attempt < 200 && !stopping; attempt++) {
    address = output.includes('diagnostic adapters ready') && output.match(/127\.0\.0\.1:\d+/)?.[0]
    if (address) break
    assert.equal(child.exitCode, null, `Native startup failed: ${output}`)
    await delay(50)
  }
  assert.ok(address, 'Native settings fixture did not become ready')
  const base = `http://${address}`
  const date = String(Math.floor(Date.now() / 1000))
  const user = JSON.stringify({ id: 42, first_name: 'Settings fixture' })
  const key = createHmac('sha256', 'WebAppData').update(bot).digest()
  const hash = createHmac('sha256', key).update(`auth_date=${date}\nuser=${user}`).digest('hex')
  const login = await fetch(base + '/api/v1/auth/token', {
    method: 'POST', headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ init_data: new URLSearchParams({ auth_date: date, user, hash }).toString() }),
  })
  assert.equal(login.status, 200)
  const token = await login.json()
  vite = await createServer({
    root, envDir: false, cacheDir: path.join(directory, 'vite-cache'),
    server: { host: '127.0.0.1', port: 0, open: false, proxy: { '/api': { target: base } } },
    plugins: [{
      name: 'settings-fixture-login',
      configureServer(server) {
        server.middlewares.use('/__settings_fixture/login', (_request, response) => {
          response.setHeader('content-type', 'application/json')
          response.setHeader('cache-control', 'no-store')
          response.end(JSON.stringify(token))
        })
      },
    }],
  })
  await vite.listen()
  console.log(`Settings SPA fixture ready: http://127.0.0.1:${vite.httpServer.address().port}`)
  console.log('Bootstrap: fetch /__settings_fixture/login, store access_token in localStorage, then open /settings.')
  await stopped
} finally {
  await vite?.close()
  if (child && child.exitCode === null) {
    child.kill('SIGTERM')
    await Promise.race([exited, delay(5000)])
    if (child.exitCode === null) { child.kill('SIGKILL'); await exited }
  }
  await rm(directory, { recursive: true, force: true })
}
