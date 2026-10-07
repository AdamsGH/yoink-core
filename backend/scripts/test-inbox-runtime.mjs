#!/usr/bin/env node
import { options, report, SCENARIOS } from './inbox-fixture/helpers.mjs';
import { withRuntime } from './inbox-fixture/runtime.mjs';
import { routing, httpContracts } from './inbox-fixture/http-scenarios.mjs';
import { telegramContracts, shutdownContracts } from './inbox-fixture/telegram-scenarios.mjs';
import { storageContracts, importContracts } from './inbox-fixture/storage-scenarios.mjs';
import { spaContracts } from './inbox-fixture/spa-scenario.mjs';
import { groupContracts } from './inbox-fixture/group-scenario.mjs';
import { deadlineContracts } from './inbox-fixture/deadline-scenario.mjs';

const config = options(process.argv.slice(2));
const selected = config.scenarios ? config.scenarios.split(',') : SCENARIOS;
const failures = [];
let passed = 0;
try {
  await withRuntime(config, async context => {
    for (const [name, scenario] of Object.entries({ routing, httpContracts, telegramContracts, groupContracts, shutdownContracts, deadlineContracts, storageContracts, importContracts, spaContracts })) {
      if (!selected.includes(name)) continue;
      try { if (name !== 'routing') await context.ensureReady(); await scenario(context); passed++; console.log(`PASS actual-process ${name}`); }
      catch (error) {
        failures.push(name);
        console.error(`FAIL actual-process ${name}\n${report(error, [context.secret, context.telegramToken])}`);
        console.error(await context.diagnostics());
        // Continue independent gates, but never convert a failed scenario into a
        // pass. A failed native stop remains reported even if forced cleanup works.
        context.trusted.holdRepository = false; context.trusted.holdStars = false; context.trusted.deadlineStars = false; context.proxy.dropSend = false; context.proxy.dropEdit = false;
        try {
          await context.db.batch("UPDATE users SET role=CASE id WHEN 42 THEN 'owner' ELSE 'user' END,ban_until=NULL WHERE id IN (42,43); DELETE FROM user_permissions WHERE user_id=43 AND plugin='inbox' AND feature='ingest';");
          await context.stop(); await context.launch(); await context.ready();
        }
        catch (reset) { throw new Error(`cannot restore fixture isolation after ${name}`, { cause: reset }); }
      }
    }
  });
} catch (error) { failures.push('runtime/cleanup'); console.error(report(error)); }
console.log(JSON.stringify({ passed, failed: failures, selected, notRunScenarios: SCENARIOS.filter(name => !selected.includes(name)), acceptance: failures.length ? 'failed' : selected.length < SCENARIOS.length ? 'selected-process-gates-passed' : 'implemented-process-gates-passed', remainingGates: ['full PostgreSQL import suite with immutable legacy images', 'admitted confined-worker shutdown with live child'] }));
if (failures.length) process.exitCode = 1;
