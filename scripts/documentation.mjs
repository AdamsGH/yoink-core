import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { lstat, mkdir, mkdtemp, readFile, readdir, realpath, rename, rm, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const repositoryRoot = fileURLToPath(new URL('../', import.meta.url));
const repositoryUrl = 'https://github.com/AdamsGH/yoink-core';
const manifestName = '.yoink-wiki.json';

export function prose(source) {
  let fence;
  return source.split('\n').map((line) => {
    const marker = /^\s*(`{3,}|~{3,})/.exec(line);
    if (marker) {
      if (!fence) fence = marker[1];
      else if (marker[1][0] === fence[0] && marker[1].length >= fence.length) fence = undefined;
      return '';
    }
    return fence ? '' : line;
  }).join('\n');
}

export function headingIds(source) {
  const ids = new Set();
  for (const match of prose(source).matchAll(/^#{1,6}\s+(.+?)\s*#*\s*$/gm)) {
    const base = match[1].replace(/<[^>]*>/g, '').toLowerCase()
      .replace(/[^\p{L}\p{N}\p{M}\s_-]/gu, '').replace(/\s/g, '-');
    let id = base;
    for (let suffix = 1; ids.has(id); suffix++) id = `${base}-${suffix}`;
    ids.add(id);
  }
  return ids;
}

export function links(source) {
  return [...prose(source).matchAll(/\[[^\]\n]*\]\(([^\s)]+)\)|^\s*\[[^\]\n]+\]:\s*(\S+)/gm)]
    .map((match) => match[1] ?? match[2]);
}

function localLink(root, file, link) {
  if (/^[a-z][a-z0-9+.-]*:|^\/\//i.test(link)) return null;
  const [location, fragment] = link.split('#', 2);
  const relative = decodeURIComponent(location.split('?', 1)[0]);
  const target = path.resolve(root, path.dirname(file), relative || path.basename(file));
  const name = path.relative(root, target).split(path.sep).join('/');
  assert.ok(name !== '..' && !name.startsWith('../') && !path.isAbsolute(name), `${file}: link escapes repository: ${link}`);
  return { target, name, fragment };
}

export async function navigation(root = repositoryRoot) {
  const canonicalRoot = await realpath(root);
  const groups = JSON.parse(await readFile(path.join(root, 'docs/navigation.json'), 'utf8'));
  const pages = groups.flatMap((group) => group.pages);
  const slugs = new Set(['_sidebar']);
  const sources = new Set();
  for (const page of pages) {
    assert.match(page.source, /^docs\/(?!agents\/).+\.md$/, 'Wiki sources must be public documents');
    assert.ok(!page.source.split('/').includes('..'), 'Wiki source escapes docs');
    assert.match(page.slug, /^[A-Za-z0-9][A-Za-z0-9-]*$/, 'Invalid Wiki page name');
    assert.ok(!slugs.has(page.slug.toLowerCase()), `Duplicate Wiki page: ${page.slug}`);
    assert.ok(!sources.has(page.source), `Duplicate Wiki source: ${page.source}`);
    slugs.add(page.slug.toLowerCase());
    sources.add(page.source);
    assert.equal(await realpath(path.join(root, page.source)), path.join(canonicalRoot, page.source), 'Wiki sources must not use symlinks');
    await readFile(path.join(root, page.source), 'utf8');
  }
  assert.ok(pages.some((page) => page.slug === 'Home'), 'Wiki needs a Home page');
  async function visit(directory) {
    for (const entry of await readdir(path.join(root, directory), { withFileTypes: true })) {
      const file = `${directory}/${entry.name}`;
      if (file === 'docs/agents') continue;
      if (entry.isDirectory()) await visit(file);
      else if (entry.name.endsWith('.md')) assert.ok(sources.has(file), `Public page is absent from navigation: ${file}`);
    }
  }
  await visit('docs');
  return { groups, pages };
}

export async function checkDocumentation(root = repositoryRoot, files) {
  await navigation(root);
  files ??= [...new Set([
    ...execFileSync('git', ['ls-files', '--recurse-submodules', '--', '*.md'], { cwd: root, encoding: 'utf8' }).trim().split('\n'),
    ...execFileSync('git', ['ls-files', '--others', '--exclude-standard', '--', '*.md'], { cwd: root, encoding: 'utf8' }).trim().split('\n'),
  ].filter(Boolean))];
  let checked = 0;
  let pages = 0;
  const errors = [];
  for (const file of files) {
    if (!(await stat(path.join(root, file)).catch(() => null))) continue;
    pages++;
    const source = await readFile(path.join(root, file), 'utf8');
    for (const link of links(source)) {
      try {
        const local = localLink(root, file, link);
        if (!local) continue;
        assert.ok(await stat(local.target).catch(() => null), `${file}: missing link target ${link}`);
        if (local.fragment && local.name.endsWith('.md')) {
          const headings = headingIds(await readFile(local.target, 'utf8'));
          assert.ok(headings.has(decodeURIComponent(local.fragment)), `${file}: missing heading ${link}`);
        }
        checked++;
      } catch (error) {
        errors.push(error);
      }
    }
  }
  if (errors.length) throw new AggregateError(errors, errors.map((error) => error.message).join('\n'));
  return { pages, links: checked };
}

export async function wikiPages(root = repositoryRoot) {
  const { groups, pages } = await navigation(root);
  const bySource = new Map(pages.map((page) => [page.source, page.slug]));
  const output = new Map();
  for (const page of pages) {
    let source = await readFile(path.join(root, page.source), 'utf8');
    const types = new Map();
    for (const link of links(source)) {
      const local = localLink(root, page.source, link);
      if (local) types.set(local.name, (await stat(local.target)).isDirectory() ? 'tree' : 'blob');
    }
    function rewrite(link) {
      const local = localLink(root, page.source, link);
      if (!local) return link;
      assert.ok(!local.name.startsWith('docs/agents/'), `${page.source}: Wiki must not link agent instructions`);
      const fragment = local.fragment ? `#${local.fragment}` : '';
      const slug = bySource.get(local.name);
      if (slug) return `${slug}${fragment}`;
      return `${repositoryUrl}/${types.get(local.name)}/master/${local.name.split('/').map(encodeURIComponent).join('/')}${fragment}`;
    }
    let fence;
    source = source.split('\n').map((line) => {
      const marker = /^\s*(`{3,}|~{3,})/.exec(line);
      if (marker) {
        if (!fence) fence = marker[1];
        else if (marker[1][0] === fence[0] && marker[1].length >= fence.length) fence = undefined;
        return line;
      }
      if (fence) return line;
      return line.replace(/(\[[^\]\n]*\]\()([^\s)]+)(\))/g, (_, before, link, after) => `${before}${rewrite(link)}${after}`)
        .replace(/^(\s*\[[^\]\n]+\]:\s*)(\S+)/, (_, before, link) => `${before}${rewrite(link)}`);
    }).join('\n');
    output.set(`${page.slug}.md`, source);
  }
  output.set('_Sidebar.md', groups.map((group) => `### ${group.title}\n\n${group.pages.map((page) => `- [${page.title}](${page.slug})`).join('\n')}`).join('\n\n') + '\n');
  return output;
}

export async function exportWiki(destination, root = repositoryRoot) {
  const output = path.resolve(destination);
  let ancestor = output;
  const missing = [];
  while (!(await lstat(ancestor).catch((error) => {
    if (error.code !== 'ENOENT') throw error;
    return null;
  }))) {
    missing.unshift(path.basename(ancestor));
    ancestor = path.dirname(ancestor);
  }
  const canonicalOutput = path.join(await realpath(ancestor), ...missing);
  const relative = path.relative(await realpath(root), canonicalOutput);
  assert.ok(relative !== '' && relative !== 'docs' && !relative.startsWith(`docs${path.sep}`), 'Export outside the documentation source directory');
  const pages = await wikiPages(root);
  let previous = [];
  const destinationInfo = await lstat(output).catch((error) => {
    if (error.code !== 'ENOENT') throw error;
    return null;
  });
  assert.ok(!destinationInfo || destinationInfo.isDirectory(), 'Export destination must be a real directory');
  const entries = await readdir(output).catch((error) => {
    if (error.code !== 'ENOENT') throw error;
    return [];
  });
  if (entries.length) {
    const manifest = JSON.parse(await readFile(path.join(output, manifestName), 'utf8'));
    previous = manifest.files;
    assert.ok(Array.isArray(previous) && previous.every((name) => /^[A-Za-z0-9_-]+\.md$/.test(name)), 'Invalid Wiki export manifest');
    assert.ok(entries.every((name) => name === manifestName || previous.includes(name)), 'Export directory contains unowned files');
    for (const name of entries) assert.ok((await lstat(path.join(output, name))).isFile(), 'Export entries must be regular files');
  }
  await mkdir(output, { recursive: true });
  const staging = await mkdtemp(path.join(path.dirname(output), '.yoink-wiki-'));
  try {
    for (const [name, source] of pages) await writeFile(path.join(staging, name), source);
    await writeFile(path.join(staging, manifestName), JSON.stringify({ files: [...pages.keys()] }, null, 2) + '\n');
    for (const name of pages.keys()) await rename(path.join(staging, name), path.join(output, name));
    for (const name of previous) if (!pages.has(name)) await rm(path.join(output, name), { force: true });
    await rename(path.join(staging, manifestName), path.join(output, manifestName));
  } finally {
    await rm(staging, { recursive: true, force: true });
  }
  return pages.size;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [command, destination] = process.argv.slice(2);
  if (command === 'check' && !destination) {
    const result = await checkDocumentation();
    await wikiPages();
    console.log(`Documentation: ${result.links} local links checked in ${result.pages} pages; Wiki navigation valid.`);
  } else if (command === 'wiki' && destination) {
    await checkDocumentation();
    console.log(`Wiki: ${await exportWiki(destination)} pages exported to ${path.resolve(destination)}.`);
  } else {
    throw new Error('Usage: node scripts/documentation.mjs check | wiki OUTPUT_DIRECTORY');
  }
}
