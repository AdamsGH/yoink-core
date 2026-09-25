import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { checkI18n } from './check-i18n.mjs';

function fixture({ en = {}, ru = {}, source = '' } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'yoink-i18n-'));
  const frontendRoot = path.join(root, 'frontend');
  fs.mkdirSync(path.join(frontendRoot, 'src/locales'), { recursive: true });
  fs.mkdirSync(path.join(frontendRoot, 'src'), { recursive: true });
  fs.writeFileSync(path.join(frontendRoot, 'src/locales/en.json'), JSON.stringify(en));
  fs.writeFileSync(path.join(frontendRoot, 'src/locales/ru.json'), JSON.stringify(ru));
  if (source) fs.writeFileSync(path.join(frontendRoot, 'src/sample.tsx'), source);
  return {
    root,
    run: () => checkI18n({ frontendRoot, repoRoot: root }),
    cleanup: () => fs.rmSync(root, { recursive: true, force: true }),
  };
}

test('reports a source key missing in both locales', () => {
  const f = fixture({ source: "t('settings.absent')" });
  try {
    const { errors } = f.run();
    assert.equal(errors.length, 2);
    assert.ok(errors.some((error) => error.includes('en is missing translation key "settings.absent"')));
    assert.ok(errors.some((error) => error.includes('ru is missing translation key "settings.absent"')));
  } finally {
    f.cleanup();
  }
});

test('reports a key missing from one locale resource', () => {
  const f = fixture({
    en: { settings: { title: 'Settings' } },
    ru: { settings: {} },
    source: "t('settings.title')",
  });
  try {
    const { errors } = f.run();
    assert.ok(errors.some((error) => error === 'Locale ru is missing resource key: settings.title'));
  } finally {
    f.cleanup();
  }
});

test('rejects empty translation values', () => {
  const f = fixture({
    en: { settings: { title: 'Settings' } },
    ru: { settings: { title: '  ' } },
    source: "t('settings.title')",
  });
  try {
    const { errors } = f.run();
    assert.ok(errors.some((error) => error === 'Locale ru has a blank or non-string value for: settings.title'));
  } finally {
    f.cleanup();
  }
});

test('accepts valid literals and straightforward conditional key strings', () => {
  const f = fixture({
    en: { settings: { first: 'First', second: 'Second' } },
    ru: { settings: { first: 'Первый', second: 'Второй' } },
    source: "t(flag ? 'settings.first' : 'settings.second')",
  });
  try {
    const { errors, sourceKeys } = f.run();
    assert.deepEqual(errors, []);
    assert.deepEqual(sourceKeys.map(({ key }) => key), ['settings.first', 'settings.second']);
  } finally {
    f.cleanup();
  }
});

test('does not treat comments or non-call strings as translation keys', () => {
  const f = fixture({
    en: { settings: { title: 'Settings' } },
    ru: { settings: { title: 'Настройки' } },
    source: [
      "// t('settings.from_comment')",
      "const text = 't(\\'settings.not_a_call\\')';",
      "const option = { label: 'settings.not_a_key' };",
      "t('settings.title');",
    ].join('\n'),
  });
  try {
    const { errors, sourceKeys } = f.run();
    assert.deepEqual(errors, []);
    assert.deepEqual(sourceKeys.map(({ key }) => key), ['settings.title']);
  } finally {
    f.cleanup();
  }
});

test('extracts literal navigation manifest keys', () => {
  const f = fixture({
    en: { nav: { settings: 'Settings' } },
    ru: { nav: { settings: 'Настройки' } },
    source: "export default [{ i18nKey: 'nav.settings' }];",
  });
  try {
    const { errors, sourceKeys } = f.run();
    assert.deepEqual(errors, []);
    assert.deepEqual(sourceKeys.map(({ key }) => key), ['nav.settings']);
  } finally {
    f.cleanup();
  }
});

test('reports nonliteral expressions as a bounded static coverage limitation', () => {
  const f = fixture({
    en: { settings: { fallback: 'Fallback' } },
    ru: { settings: { fallback: 'По умолчанию' } },
    source: "t('settings.' + selected)",
  });
  try {
    const { errors, dynamic } = f.run();
    assert.deepEqual(errors, []);
    assert.equal(dynamic.length, 1);
    assert.equal(dynamic[0].expression, "'settings.' + selected");
  } finally {
    f.cleanup();
  }
});

test('checks asserted and parenthesized literal keys rather than skipping them', () => {
  const f = fixture({ source: "t(('settings.absent' as const)); t('settings.second' satisfies string)" });
  try {
    const { errors, sourceKeys, dynamic } = f.run();
    assert.equal(errors.length, 4);
    assert.deepEqual(sourceKeys.map(({ key }) => key), ['settings.absent', 'settings.second']);
    assert.deepEqual(dynamic, []);
  } finally {
    f.cleanup();
  }
});

test('checks core and plugin root manifests as well as plugin source pages', () => {
  const f = fixture();
  try {
    fs.writeFileSync(path.join(f.root, 'frontend/manifest.tsx'), "const item = { 'i18nKey': 'nav.core' }");
    const pluginRoot = path.join(f.root, 'plugins/example/frontend');
    fs.mkdirSync(path.join(pluginRoot, 'src'), { recursive: true });
    fs.writeFileSync(path.join(pluginRoot, 'manifest.tsx'), "const item = { i18nKey: 'nav.plugin' }");
    fs.writeFileSync(path.join(pluginRoot, 'src/Page.tsx'), "t('plugin.title')");
    const { errors, sourceKeys } = f.run();
    assert.equal(errors.length, 6);
    assert.deepEqual(sourceKeys.map(({ key }) => key).sort(), ['nav.core', 'nav.plugin', 'plugin.title']);
  } finally {
    f.cleanup();
  }
});

test('rejects missing interpolation parameters in a translated string', () => {
  const f = fixture({
    en: { result: 'Loaded {{count}} items for {{name}}' },
    ru: { result: 'Загружено {{count}} записей' },
  });
  try {
    assert.deepEqual(f.run().errors, ['Locale interpolation parameters differ for: result']);
  } finally {
    f.cleanup();
  }
});

test('accepts reordered, formatted and unescaped interpolation parameters', () => {
  const f = fixture({
    en: { result: '{{count, number}} items for {{- name}} ({{count}})' },
    ru: { result: '{{name}}: записей {{count}}' },
  });
  try {
    assert.deepEqual(f.run().errors, []);
  } finally {
    f.cleanup();
  }
});

test('rejects non-string resource values', () => {
  const f = fixture({ en: { title: 123 }, ru: { title: null } });
  try {
    assert.equal(f.run().errors.length, 2);
    assert.ok(f.run().errors.every((error) => error.includes('blank or non-string')));
  } finally {
    f.cleanup();
  }
});
