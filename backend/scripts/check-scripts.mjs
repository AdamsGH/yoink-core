// Parse every project-owned JavaScript tool and fixture module without executing it.
import { readdir } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

let checked = 0;
async function checkDirectory(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  for (const entry of entries.sort((a, b) => a.name.localeCompare(b.name))) {
    const file = new URL(entry.name + (entry.isDirectory() ? '/' : ''), directory);
    if (entry.isDirectory()) {
      await checkDirectory(file);
    } else if (entry.isFile() && entry.name.endsWith('.mjs')) {
      const result = spawnSync(process.execPath, ['--check', fileURLToPath(file)], {
        encoding: 'utf8', timeout: 10000, maxBuffer: 65536,
        stdio: ['ignore', 'pipe', 'pipe'],
      });
      if (result.error || result.status !== 0) {
        throw new Error(`Syntax check failed: ${fileURLToPath(file)}\n${result.error?.message ?? result.stderr}`);
      }
      checked++;
    }
  }
}
await checkDirectory(new URL('./', import.meta.url));
console.log(`Checked ${checked} JavaScript tool and fixture modules.`);
