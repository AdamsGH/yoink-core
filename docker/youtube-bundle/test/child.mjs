import { appendFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const [directory, name, mode] = process.argv.slice(2);
writeFileSync(join(directory, name), String(process.pid));
process.on('SIGTERM', () => {
  if (mode === 'ignore') return;
  appendFileSync(join(directory, 'stopped'), `${name}\n`);
  process.exit(0);
});
setInterval(() => {}, 1000);
