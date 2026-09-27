import assert from 'node:assert/strict';
import { readdir, readFile, stat } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const files = ['backend/README.md'];
for (const directory of ['docs/backend', 'docs/decisions']) {
  for (const entry of await readdir(path.join(root, directory))) {
    if (entry.endsWith('.md')) files.push(`${directory}/${entry}`);
  }
}
let checked = 0;
for (const file of files) {
  const source = await readFile(path.join(root, file), 'utf8');
  for (const match of source.matchAll(/\[[^\]\n]*\]\(([^)\s]+)\)/g)) {
    const link = match[1];
    if (/^[a-z][a-z0-9+.-]*:/i.test(link)) continue;
    const [relative, anchor] = link.split('#', 2);
    const target = relative ? path.resolve(root, path.dirname(file), decodeURIComponent(relative)) : path.join(root, file);
    assert.ok(target.startsWith(root), `${file}: link escapes repository: ${link}`);
    const metadata = await stat(target).catch(() => null);
    assert.ok(metadata, `${file}: missing link target ${link}`);
    if (anchor && target.endsWith('.md')) {
      const text = await readFile(target, 'utf8');
      const headings = [...text.matchAll(/^#{1,6}\s+(.+)$/gm)].map((heading) => heading[1].toLowerCase().replace(/[^\p{L}\p{N}\s_-]/gu, '').trim().replace(/\s/g, '-'));
      assert.ok(headings.includes(decodeURIComponent(anchor)), `${file}: missing heading ${link}`);
    }
    checked++;
  }
}
console.log(`Documentation links: ${checked} checked in ${files.length} pages.`);
