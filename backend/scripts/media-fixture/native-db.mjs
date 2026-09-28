import assert from 'node:assert/strict';
import net from 'node:net';

export function formatFixtureRows(rows) {
  return rows.map(row => row.map(cell => cell === null ? '' : String(cell)).join('|')).join('\n');
}

export function createNativeDbClient({ getAppContainerId, getFixturePort, fixtureToken, volumeName, fixtureImage, helperContainer, timeout = 7000, maxPayload = 1024 * 1024 }) {
  async function request(sql, mode = 'query') {
    const payload = `${JSON.stringify({ token: fixtureToken, sql, mode })}\n`;
    assert(Buffer.byteLength(payload) <= maxPayload, 'fixture request exceeds bound');
    const response = getAppContainerId()
      ? await requestSocket(getFixturePort(), payload, timeout, maxPayload)
      : JSON.parse(await helperContainer(['--network', 'none', '--mount', `type=volume,source=${volumeName},target=/state`,
        '--env', 'YOINK_RUST_DATABASE_PATH=/state/private/state.db', '--env', `YOINK_FIXTURE_TOKEN=${fixtureToken}`,
        '--entrypoint', 'yoink-fixture-server', fixtureImage, 'sql'], 30000, payload));
    assert.equal(response.ok, true, `fixture SQL ${mode} failed: ${response.error ?? 'unknown error'}`);
    return response;
  }
  return {
    request,
    async query(sql) { return formatFixtureRows((await request(sql, 'query')).rows); },
    async batch(sql) { await request(sql, 'batch'); return ''; },
  };
}

function requestSocket(port, payload, timeout, maxPayload) {
  return new Promise((resolve, reject) => {
    const socket = net.connect(port, '127.0.0.1');
    let output = '';
    let settled = false;
    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.destroy();
      if (error) reject(error);
      else resolve(value);
    };
    const timer = setTimeout(() => finish(new Error('fixture SQL request timed out')), timeout);
    socket.setEncoding('utf8');
    socket.on('connect', () => socket.end(payload));
    socket.on('data', chunk => {
      output += chunk;
      if (Buffer.byteLength(output) > maxPayload) finish(new Error('fixture SQL response exceeded limit'));
      const newline = output.indexOf('\n');
      if (newline >= 0) {
        try { finish(null, JSON.parse(output.slice(0, newline))); }
        catch (error) { finish(new Error('fixture SQL response was invalid JSON', { cause: error })); }
      }
    });
    socket.once('error', error => finish(error));
    socket.once('end', () => { if (!settled) finish(new Error('fixture SQL response ended before newline')); });
  });
}
