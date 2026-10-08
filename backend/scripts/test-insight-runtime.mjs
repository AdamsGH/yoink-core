#!/usr/bin/env node
import { options, GATES, GAPS, report } from './insight-fixture/helpers.mjs';
import { withRuntime } from './insight-fixture/runtime.mjs';
import * as contracts from './insight-fixture/scenarios.mjs';
import * as advanced from './insight-fixture/advanced.mjs';
import { oauth, oauthStart } from './insight-fixture/oauth.mjs';
import { spa } from './insight-fixture/spa.mjs';
import { forcedReceipt } from './insight-fixture/forced-receipt.mjs';
import * as positive from './insight-fixture/tls-scenarios.mjs';
const failures=[],passed=[];let config;
try {
  config=options(process.argv.slice(2));
  await withRuntime(config,async c=>{
    for(const name of config.selected){
      try {if(name!=='routing')await c.ensureReady({tls:name.startsWith('positive')});await ({...contracts,...advanced,...positive,oauth,oauthStart,spa,forcedReceipt})[name](c);passed.push(name);console.log(`PASS actual-process ${name}`);}
      catch(e){failures.push(name);console.error(`FAIL actual-process ${name}\n${c.sanitize(e)}`);
        // Native failure stays failed even if forced teardown later succeeds.
        // Abort rather than silently resetting state shared with later gates.
        throw new Error(`actual-process gate ${name} failed`,{cause:e});
      }
    }
  });
}catch(e){failures.push('runtime/setup/cleanup');console.error(report(e));}
console.log(JSON.stringify({passed,failed:failures,selected:config?.selected??[],notRun:GATES.filter(x=>!passed.includes(x)&&!failures.includes(x)),acceptance:failures.length?'failed':'selected-process-gates-passed',coordinatorInterfaceBlockers:GAPS}));
if(failures.length)process.exitCode=1;
