import { randomBytes } from 'node:crypto';
import { run } from './process.mjs';

export async function nativeRequest(binary, dbPath, sql, mode = 'query') {
  const token = randomBytes(24).toString('hex');
  const response = await run(binary, ['sql'], {
    timeout: 15_000,
    env: {
      PATH: process.env.PATH,
      HOME: process.env.HOME,
      YOINK_RUST_DATABASE_PATH: dbPath,
      YOINK_RUST_TELEGRAM_ENABLED: 'false',
      YOINK_RUST_BOT_TOKEN: '',
      YOINK_RUST_MEDIA_ROOT: '',
      YOINK_FIXTURE_TOKEN: token,
    },
    input: `${JSON.stringify({ token, sql, mode })}\n`,
  });
  let parsed;
  try { parsed = JSON.parse(response.stdout); }
  catch { throw new Error(`native fixture server returned invalid JSON: ${response.stdout.slice(0, 1000)}`); }
  if (!parsed.ok) throw new Error(`native SQL ${mode} failed: ${parsed.error ?? 'unknown error'}`);
  return parsed;
}
