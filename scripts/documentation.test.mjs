import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtemp, mkdir, readFile, readdir, rm, stat, symlink, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { checkDocumentation, exportWiki, headingIds, navigation, repositoryRoot, wikiPages } from './documentation.mjs';
import { images } from './container-images.mjs';

async function fixture(t) {
  const root = await mkdtemp(path.join(os.tmpdir(), 'yoink-docs-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(path.join(root, 'docs/agents'), { recursive: true });
  await writeFile(path.join(root, 'docs/README.md'), '# Home\n\n[Topic](topic.md#details)\n');
  await writeFile(path.join(root, 'docs/topic.md'), '# Topic\n\n## Details\n\n[Command](../justfile)\n');
  await writeFile(path.join(root, 'docs/agents/rules.md'), '# Private agent instructions\n');
  await writeFile(path.join(root, 'justfile'), 'default:\n    echo docs\n');
  const groups = [{ title: 'Guide', pages: [
    { title: 'Home', source: 'docs/README.md', slug: 'Home' },
    { title: 'Topic', source: 'docs/topic.md', slug: 'Topic' },
  ] }];
  const save = () => writeFile(path.join(root, 'docs/navigation.json'), JSON.stringify(groups));
  await save();
  return { root, groups, save };
}

test('local targets and fragments are checked while fenced examples are ignored', async (t) => {
  const { root } = await fixture(t);
  await writeFile(path.join(root, 'docs/README.md'), '# Home\n\n[Topic](topic.md#details)\n\n```md\n[Example](missing.md)\n```\n');
  assert.equal((await checkDocumentation(root, ['docs/README.md'])).links, 1);
  assert.equal((await checkDocumentation(root, ['docs/README.md', 'deleted.md'])).pages, 1);
  await writeFile(path.join(root, 'docs/README.md'), '# Home\n\n[Bad](topic.md#absent)\n');
  await assert.rejects(checkDocumentation(root, ['docs/README.md']), /missing heading/);
  await writeFile(path.join(root, 'docs/README.md'), '# Home\n\n[Bad](missing.md)\n');
  await assert.rejects(checkDocumentation(root, ['docs/README.md']), /missing link target/);
  await writeFile(path.join(root, 'docs/README.md'), '# Home\n\n[Bad](../../outside.md)\n');
  await assert.rejects(checkDocumentation(root, ['docs/README.md']), /escapes repository/);
});

test('heading anchors handle repeated headings and ignore code fences', () => {
  assert.deepEqual([...headingIds('# Name\n## Name\n```md\n# Hidden\n```\n## A `code` name\n')], ['name', 'name-1', 'a-code-name']);
});

test('Wiki includes public pages, rewrites source links, and preserves code examples', async (t) => {
  const { root } = await fixture(t);
  await writeFile(path.join(root, 'docs/README.md'), '# Home\n\n[Topic](topic.md#details)\n\n```md\n[Example](topic.md)\n```\n');
  const pages = await wikiPages(root);
  assert.deepEqual([...pages.keys()], ['Home.md', 'Topic.md', '_Sidebar.md']);
  assert.match(pages.get('Home.md'), /\[Topic\]\(Topic#details\)/);
  assert.match(pages.get('Home.md'), /\[Example\]\(topic\.md\)/);
  assert.match(pages.get('Topic.md'), /github\.com\/AdamsGH\/yoink-core\/blob\/master\/justfile/);
  assert.ok(![...pages.values()].join('\n').includes('Private agent instructions'));
});

test('reference links use the same local and Wiki target rules', async (t) => {
  const { root } = await fixture(t);
  await writeFile(path.join(root, 'docs/README.md'), '# Home\n\n[Topic][details]\n\n[details]: topic.md#details\n');
  assert.equal((await checkDocumentation(root, ['docs/README.md'])).links, 1);
  assert.match((await wikiPages(root)).get('Home.md'), /\[details\]: Topic#details/);
});

test('agent sources and agent links cannot enter the Wiki', async (t) => {
  const { root, groups, save } = await fixture(t);
  await writeFile(path.join(root, 'docs/README.md'), '# Home\n\n[Rules](agents/rules.md)\n');
  await assert.rejects(wikiPages(root), /must not link agent instructions/);
  groups[0].pages.push({ title: 'Rules', source: 'docs/agents/rules.md', slug: 'Rules' });
  await save();
  await assert.rejects(navigation(root), /must be public documents/);
});

test('a public source cannot alias agent content through a symlink', async (t) => {
  const { root } = await fixture(t);
  await rm(path.join(root, 'docs/topic.md'));
  await symlink(path.join(root, 'docs/agents/rules.md'), path.join(root, 'docs/topic.md'));
  await assert.rejects(wikiPages(root), /must not use symlinks/);
});

test('unlisted public pages and case-insensitive Wiki name collisions fail', async (t) => {
  const { root, groups, save } = await fixture(t);
  groups[0].pages[1].slug = 'home';
  await save();
  await assert.rejects(navigation(root), /Duplicate Wiki page/);
  groups[0].pages[1].slug = 'Topic';
  await save();
  await writeFile(path.join(root, 'docs/unlisted.md'), '# Unlisted\n');
  await assert.rejects(navigation(root), /absent from navigation/);
});

test('re-export removes retired owned pages without deleting unowned files', async (t) => {
  const { root, groups, save } = await fixture(t);
  const output = path.join(root, 'export');
  await exportWiki(output, root);
  groups[0].pages[1].slug = 'Renamed';
  await save();
  await exportWiki(output, root);
  assert.ok(!(await readdir(output)).includes('Topic.md'));
  assert.ok((await readdir(output)).includes('Renamed.md'));
  await writeFile(path.join(output, 'Human.md'), 'Keep me\n');
  await assert.rejects(exportWiki(output, root), /unowned files/);
  assert.equal(await readFile(path.join(output, 'Human.md'), 'utf8'), 'Keep me\n');
});

test('unsafe export targets, manifest paths and symlinks are rejected', async (t) => {
  const { root } = await fixture(t);
  await assert.rejects(exportWiki(path.join(root, 'docs'), root), /outside the documentation/);
  await symlink(path.join(root, 'docs'), path.join(root, 'docs-alias'));
  await assert.rejects(exportWiki(path.join(root, 'docs-alias/export'), root), /outside the documentation/);
  const output = path.join(root, 'export');
  await exportWiki(output, root);
  await writeFile(path.join(output, '.yoink-wiki.json'), JSON.stringify({ files: ['../justfile'] }));
  await assert.rejects(exportWiki(output, root), /Invalid Wiki export manifest/);
  await writeFile(path.join(output, '.yoink-wiki.json'), JSON.stringify({ files: ['Home.md', 'Topic.md', '_Sidebar.md'] }));
  await rm(path.join(output, 'Topic.md'));
  await symlink(path.join(root, 'justfile'), path.join(output, 'Topic.md'));
  await assert.rejects(exportWiki(output, root), /must be regular files/);
  await symlink(output, path.join(root, 'alias'));
  await assert.rejects(exportWiki(path.join(root, 'alias'), root), /real directory/);
});

test('documented stored-setting values match the native parser, not command overrides', async () => {
  const source = await readFile(path.join(repositoryRoot, 'backend/crates/yoink-downloads/src/settings.rs'), 'utf8');
  const document = await readFile(path.join(repositoryRoot, 'docs/features/downloads/settings.md'), 'utf8');
  const labels = {
    Quality: 'Quality', Codec: 'Video codec', Container: 'Container', AudioCodec: 'Audio codec',
    SplitSize: 'Split presets, bytes', Keyboard: 'Keyboard layout', SubtitleLanguage: 'Subtitle language',
  };
  for (const [field, label] of Object.entries(labels)) {
    const choices = new RegExp(`Self::${field} => &\\[([\\s\\S]*?)\\]`).exec(source);
    assert.ok(choices, `Native choices missing for ${field}`);
    const row = document.split('\n').find((line) => line.startsWith(`| ${label} |`));
    assert.ok(row, `Documentation row missing for ${field}`);
    const expected = [...choices[1].matchAll(/"([^"]+)"/g)].map((match) => match[1]).sort();
    const actual = [...row.matchAll(/`([^`]+)`/g)].map((match) => match[1]).sort();
    assert.deepEqual(actual, expected, `Documentation differs from stored ${field} validation`);
  }
  const schema = JSON.parse(await readFile(path.join(repositoryRoot, 'backend/crates/yoink-http/src/configuration-openapi.json'), 'utf8'));
  assert.ok(schema.paths['/api/v1/dl/settings'].patch);
  assert.match(document, /`\/api\/v1\/dl\/settings`/);
});

test('publication documentation covers exactly the current image matrix', async () => {
  const document = await readFile(path.join(repositoryRoot, 'docs/operations/publication.md'), 'utf8');
  const documented = [...document.matchAll(/^\| `([^`]+)` \|/gm)].map((match) => match[1]).sort();
  assert.deepEqual(documented, images.map((image) => image.name).sort());
});

test('literal Rust documentation inputs remain tracked and readable after moves', async () => {
  const files = execFileSync('git', ['ls-files', '-z', '--', 'backend', 'docs'], { cwd: repositoryRoot, encoding: 'utf8' }).split('\0').filter(Boolean);
  const tracked = new Set(files);
  let checked = 0;
  for (const file of files.filter(file => file.endsWith('.rs'))) {
    const source = await readFile(path.join(repositoryRoot, file), 'utf8');
    for (const match of source.matchAll(/include_(?:str|bytes)!\s*\(\s*"([^"\n]*docs\/[^"\n]+)"\s*\)/g)) {
      const target = path.resolve(repositoryRoot, path.dirname(file), match[1]);
      const relative = path.relative(repositoryRoot, target);
      assert(tracked.has(relative), `${file} embeds an untracked documentation input: ${relative}`);
      assert((await stat(target)).isFile(), `${file} must embed a regular documentation file`);
      await readFile(target);
      checked++;
    }
  }
  assert(checked > 0, 'Expected native Rust documentation inputs to be checked.');
});
