import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const FRONTEND_ROOT = path.resolve(SCRIPT_DIR, '..');
const REPO_ROOT = path.resolve(FRONTEND_ROOT, '..');

function listSourceFiles(root) {
  if (!fs.existsSync(root)) return [];
  const files = [];
  const visit = (directory) => {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      if (entry.name === 'node_modules' || entry.name === 'dist' || entry.name === 'build') continue;
      const fullPath = path.join(directory, entry.name);
      if (entry.isDirectory()) visit(fullPath);
      else if (/\.(?:[cm]?[jt]sx?)$/.test(entry.name)) files.push(fullPath);
    }
  };
  visit(root);
  return files;
}

function flattenResource(value, prefix = '', result = new Map()) {
  for (const [key, child] of Object.entries(value)) {
    const fullKey = prefix ? `${prefix}.${key}` : key;
    if (child !== null && typeof child === 'object' && !Array.isArray(child)) {
      flattenResource(child, fullKey, result);
    } else {
      result.set(fullKey, child);
    }
  }
  return result;
}

function staticStrings(expression) {
  if (ts.isParenthesizedExpression(expression) || ts.isAsExpression(expression)
    || ts.isTypeAssertionExpression(expression) || ts.isSatisfiesExpression(expression)
    || ts.isNonNullExpression(expression)) {
    return staticStrings(expression.expression);
  }
  if (ts.isStringLiteral(expression) || ts.isNoSubstitutionTemplateLiteral(expression)) {
    return [expression.text];
  }
  if (ts.isConditionalExpression(expression)) {
    const whenTrue = staticStrings(expression.whenTrue);
    const whenFalse = staticStrings(expression.whenFalse);
    return whenTrue && whenFalse ? [...whenTrue, ...whenFalse] : null;
  }
  return null;
}

function interpolationNames(value) {
  return [...new Set([...value.matchAll(/\{\{\s*-?\s*([^},\s]+)(?:,[^}]*)?\s*\}\}/g)].map((match) => match[1]))].sort();
}

function isTranslationCall(node) {
  if (!ts.isCallExpression(node)) return false;
  const expression = node.expression;
  return (ts.isIdentifier(expression) && expression.text === 't')
    || (ts.isPropertyAccessExpression(expression) && expression.name.text === 't');
}

function inspectSource(filePath) {
  const sourceText = fs.readFileSync(filePath, 'utf8');
  const sourceFile = ts.createSourceFile(filePath, sourceText, ts.ScriptTarget.Latest, true);
  const keys = [];
  const dynamic = [];
  const visit = (node) => {
    if (isTranslationCall(node) && node.arguments.length > 0) {
      const found = staticStrings(node.arguments[0]);
      const { line } = sourceFile.getLineAndCharacterOfPosition(node.arguments[0].getStart(sourceFile));
      if (found) {
        for (const key of found) keys.push({ key, filePath, line: line + 1 });
      } else {
        dynamic.push({ filePath, line: line + 1, expression: node.arguments[0].getText(sourceFile) });
      }
    }
    if (ts.isPropertyAssignment(node) && node.name.text === 'i18nKey') {
      const found = staticStrings(node.initializer);
      const { line } = sourceFile.getLineAndCharacterOfPosition(node.initializer.getStart(sourceFile));
      if (found) {
        for (const key of found) keys.push({ key, filePath, line: line + 1 });
      } else {
        dynamic.push({ filePath, line: line + 1, expression: node.initializer.getText(sourceFile) });
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(sourceFile);
  return { keys, dynamic };
}

export function checkI18n({ frontendRoot = FRONTEND_ROOT, repoRoot = REPO_ROOT } = {}) {
  const localePaths = {
    en: path.join(frontendRoot, 'src/locales/en.json'),
    ru: path.join(frontendRoot, 'src/locales/ru.json'),
  };
  const resources = {};
  for (const [locale, localePath] of Object.entries(localePaths)) {
    resources[locale] = flattenResource(JSON.parse(fs.readFileSync(localePath, 'utf8')));
  }

  const errors = [];
  const enKeys = new Set(resources.en.keys());
  const ruKeys = new Set(resources.ru.keys());
  for (const key of [...new Set([...enKeys, ...ruKeys])].sort()) {
    if (!enKeys.has(key)) errors.push(`Locale en is missing resource key: ${key}`);
    if (!ruKeys.has(key)) errors.push(`Locale ru is missing resource key: ${key}`);
    if (typeof resources.en.get(key) === 'string' && typeof resources.ru.get(key) === 'string'
      && JSON.stringify(interpolationNames(resources.en.get(key))) !== JSON.stringify(interpolationNames(resources.ru.get(key)))) {
      errors.push(`Locale interpolation parameters differ for: ${key}`);
    }
    for (const locale of ['en', 'ru']) {
      if (resources[locale].has(key)) {
        const value = resources[locale].get(key);
        if (typeof value !== 'string' || value.trim() === '') {
          errors.push(`Locale ${locale} has a blank or non-string value for: ${key}`);
        }
      }
    }
  }

  const roots = [
    path.join(frontendRoot, 'src'),
    ...fs.existsSync(path.join(repoRoot, 'plugins'))
      ? fs.readdirSync(path.join(repoRoot, 'plugins'), { withFileTypes: true })
          .filter((entry) => entry.isDirectory())
          .map((entry) => path.join(repoRoot, 'plugins', entry.name, 'frontend'))
      : [],
  ];
  const sourceFiles = roots.flatMap(listSourceFiles);
  const coreManifest = path.join(frontendRoot, 'manifest.tsx');
  if (fs.existsSync(coreManifest)) sourceFiles.push(coreManifest);
  const sourceKeys = [];
  const dynamic = [];
  for (const filePath of sourceFiles) {
    const result = inspectSource(filePath);
    sourceKeys.push(...result.keys);
    dynamic.push(...result.dynamic);
  }

  for (const { key, filePath, line } of sourceKeys) {
    for (const locale of ['en', 'ru']) {
      if (!resources[locale].has(key)) {
        errors.push(`${path.relative(repoRoot, filePath)}:${line}: ${locale} is missing translation key "${key}"`);
      }
    }
  }

  return { errors, sourceKeys, dynamic, sourceFiles, resources };
}

const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  const result = checkI18n();
  const verbose = process.argv.includes('--verbose');
  if (result.errors.length) {
    for (const error of result.errors.slice(0, verbose ? undefined : 40)) console.error(error);
    if (!verbose && result.errors.length > 40) console.error(`... ${result.errors.length - 40} more errors. Pass --verbose for all diagnostics.`);
    console.error(`i18n check failed with ${result.errors.length} error(s).`);
    process.exitCode = 1;
  } else {
    console.log(`i18n check passed: ${result.sourceKeys.length} statically extracted references across ${result.sourceFiles.length} source files; en/ru resources are nonblank and in parity.`);
  }
  if (result.dynamic.length) {
    console.warn(`Static coverage limit: ${result.dynamic.length} dynamic key expressions are not validated. Pass --verbose for locations.`);
    if (verbose) for (const item of result.dynamic) {
      console.warn(`  ${path.relative(REPO_ROOT, item.filePath)}:${item.line}: ${item.expression}`);
    }
  }
}
