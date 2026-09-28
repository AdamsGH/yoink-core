import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { writeFile, readFile, rm, readdir } from 'node:fs/promises';
import path from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { run, assertNoSecrets } from './process.mjs';
import { nativeRequest } from './native.mjs';

export async function tlsChecks({ fixture, sourceUrl, disposable, importer, native, backend, secrets, image }) {
  assert(fixture.name.startsWith('yoink-turso-import-'));
  const temporary = await disposable();
  const ca = path.join(temporary.sourceDir, 'ca.crt');
  const caKey = path.join(temporary.sourceDir, 'ca.key');
  const key = path.join(temporary.sourceDir, 'server.key');
  const cert = path.join(temporary.sourceDir, 'server.crt');
  const request = path.join(temporary.sourceDir, 'server.csr');
  const extensions = path.join(temporary.sourceDir, 'server.ext');
  let configured = false;
  try {
    await run('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
      '-subj', '/CN=Yoink disposable import CA', '-addext', 'basicConstraints=critical,CA:TRUE',
      '-keyout', caKey, '-out', ca]);
    await run('openssl', ['req', '-new', '-newkey', 'rsa:2048', '-nodes',
      '-subj', '/CN=Yoink disposable import server', '-keyout', key, '-out', request]);
    await writeFile(extensions, 'basicConstraints=critical,CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1\n', { mode: 0o600 });
    await run('openssl', ['x509', '-req', '-in', request, '-CA', ca, '-CAkey', caKey,
      '-CAcreateserial', '-days', '1', '-extfile', extensions, '-out', cert]);
    for (const [file, name] of [[key, 'import-server.key'], [cert, 'import-server.crt']]) {
      // Write inside the running tmpfs mount, rather than through Docker's archive-copy path.
      await run('docker', ['exec', '-i', '--user', 'postgres', fixture.name,
        'sh', '-c', `umask 077; cat > /var/lib/postgresql/data/${name}`], { input: await readFile(file) });
    }
    await fixture.query("ALTER SYSTEM SET ssl_cert_file='/var/lib/postgresql/data/import-server.crt'");
    await fixture.query("ALTER SYSTEM SET ssl_key_file='/var/lib/postgresql/data/import-server.key'");
    await fixture.query("ALTER SYSTEM SET ssl='on'");
    configured = true;
    await fixture.query('SELECT pg_reload_conf()');
    await waitSsl(fixture, 'on');

    const invoke = (url, destination, caPath) => run(importer,
      ['--destination', destination, ...(caPath ? ['--ca-cert', caPath] : [])], {
        cwd: backend, timeout: 60_000,
        env: { PATH: process.env.PATH, HOME: process.env.HOME, YOINK_IMPORT_SOURCE_URL: url },
      });
    const untrusted = path.join(temporary.destinationDir, 'untrusted.db');
    await assert.rejects(invoke(sourceUrl, untrusted), error => {
      assertNoSecrets(error.stdout + error.stderr, secrets);
      assert.equal(error.category, 'exit');
      assert(error.stderr.includes('TLS'));
      return true;
    });
    assert.deepEqual(await readdir(temporary.destinationDir), []);

    const mismatched = new URL(sourceUrl);
    mismatched.hostname = 'wrong-host.invalid';
    mismatched.searchParams.set('hostaddr', '127.0.0.1');
    await assert.rejects(invoke(mismatched.toString(), path.join(temporary.destinationDir, 'wrong-host.db'), ca), error => {
      assertNoSecrets(error.stdout + error.stderr, secrets);
      assert.equal(error.category, 'exit');
      assert(error.stderr.includes('TLS'));
      return true;
    });
    assert.deepEqual(await readdir(temporary.destinationDir), []);

    const result = await invoke(sourceUrl, temporary.db, ca);
    assertNoSecrets(result.stdout + result.stderr, secrets);
    assert(result.stdout.includes('41 tables'));
    const imported = await nativeRequest(native, temporary.db, 'SELECT CAST(id AS TEXT) FROM users WHERE id=9007199254740993');
    assert.equal(imported.rows[0][0], '9007199254740993', 'verified TLS imports the real source');
    console.log('TLS: trusted CA accepted, untrusted issuer and mismatched hostname rejected.');
    if (image) {
      const containerUrl = new URL(sourceUrl);
      containerUrl.port = '5432';
      const uid = process.getuid();
      const gid = process.getgid();
      const name = `yoink-import-image-${randomUUID()}`;
      let container;
      try {
        container = await run('docker', ['run', '--name', name, '--rm', '--pull', 'never', '--read-only',
        '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', '--user', `${uid}:${gid}`,
        '--network', `container:${fixture.name}`,
        '--tmpfs', `/tmp/yoink-work:rw,noexec,nosuid,mode=0700,uid=${uid},gid=${gid}`,
        '--mount', `type=bind,source=${temporary.destinationDir},target=/destination`,
        '--mount', `type=bind,source=${ca},target=/certs/ca.crt,readonly`,
        '--env', 'YOINK_IMPORT_SOURCE_URL', image,
        '--destination', '/destination/container.db', '--ca-cert', '/certs/ca.crt'], {
          env: { ...process.env, YOINK_IMPORT_SOURCE_URL: containerUrl.toString() }, timeout: 60_000,
        });
      } finally {
        await run('docker', ['rm', '--force', name]).catch(error => {
          if (!error.stderr?.includes('No such container')) throw error;
        });
      }
      assertNoSecrets(container.stdout + container.stderr, secrets);
      assert(container.stdout.includes('41 tables'));
      const rows = await nativeRequest(native, path.join(temporary.destinationDir, 'container.db'),
        'SELECT CAST(id AS TEXT) FROM users WHERE id=9007199254740993');
      assert.equal(rows.rows[0][0], '9007199254740993');
      console.log('Pinned-compiler importer image: read-only root, private tmpfs, verified TLS and native reopen passed.');
    }
  } finally {
    if (configured) {
      await fixture.query("ALTER SYSTEM SET ssl='off'");
      await fixture.query('SELECT pg_reload_conf()');
      await waitSsl(fixture, 'off');
    }
    await rm(temporary.root, { recursive: true, force: true });
  }
}

async function waitSsl(fixture, expected) {
  for (let attempt = 0; attempt < 50; attempt++) {
    if (await fixture.query('SHOW ssl') === expected) return;
    await delay(100);
  }
  throw new Error('fixture PostgreSQL did not reload its TLS configuration');
}
