import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { runPrivateUserScenarios } from './private-user-scenarios.mjs';

const source = await readFile(new URL('./private-user-scenarios.mjs', import.meta.url), 'utf8');
test('ordinary-user fixture uses an actual non-owner actor and served fenced choices', () => {
  assert.equal(typeof runPrivateUserScenarios, 'function');
  assert(source.includes('const actor = 43'));
  assert(source.includes("role='user',ban_until=NULL WHERE id=43"));
  for (const workflow of ['/settings', '/format', '/list', '/audio', 'ql:cut:', 'cut:go:', 'cut:q720:']) assert(source.includes(workflow));
  for (const invariant of ['restartApp', 'parseFormatReport', 'h.inspectUpload', 'h.replay', 'ownerSettings', "['restricted', 'banned']", 'show_alert']) assert(source.includes(invariant));
});
test('focused selector does not invoke unrelated provider or inline workflows', async () => {
  const runner = await readFile(new URL('../test-media.mjs', import.meta.url), 'utf8');
  assert.match(runner, /if \(scenario === 'private-user'\) summaries.push\(await runPrivateUserScenarios/);
  assert(!source.includes('inline_query'));
  assert(!source.includes('createHmac'));
});
