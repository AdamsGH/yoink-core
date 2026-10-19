import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {spawnSync} from 'node:child_process';
import {isRequestId, parseEvents, summarize} from './insight-diagnostics/report.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const options = {};
const args = process.argv.slice(2);
for (let i=0;i<args.length;i+=2) {
  if (!['--since','--request','--snapshot','--match'].includes(args[i]) || args[i+1] === undefined || Object.hasOwn(options,args[i])) throw new Error('Use --since 24h [--request UUID] [--snapshot latest|data/native-restore/file.db] [--match non-secret fragment].');
  options[args[i]] = args[i+1];
}
const since = options['--since'] || '24h', request = (options['--request'] || '').toLowerCase();
if (!/^[1-9]\d{0,2}[smh]$/.test(since) || (request && !isRequestId(request)) || (options['--match']?.length ?? 0)>256) throw new Error('Use a duration of 1..999 s/m/h, a canonical UUID and a match fragment of at most 256 characters.');
if (options['--match'] && !options['--snapshot']) throw new Error('--match requires --snapshot latest or an inactive restore basename.');
function docker(args, input) {
  const result = spawnSync('docker',args,{cwd:root,encoding:'utf8',input,timeout:30000,maxBuffer:32*1024*1024});
  if (result.error || result.status !== 0) {
    if (result.error?.code === 'ENOBUFS') throw new Error('Retained logs exceed the 32 MiB collection limit. Narrow --since, for example --since 5m.');
    const kind = /\b(PermissionError|FileNotFoundError|ValueError|JSONDecodeError|OperationalError):/.exec(result.stderr || '')?.[1] || 'unavailable';
    throw new Error(`Docker diagnostic operation failed (${args[0]}, exit=${result.status}, kind=${kind}). For snapshots, verify an inactive restore exists and the running image has Python. Keep snapshot ownership 10001 and private permissions; do not chmod it for the host. Raw stderr is withheld to protect credentials.`);
  }
  return result;
}
const current = JSON.parse(docker(['inspect','yoink']).stdout)[0];
const logs = docker(['logs','--timestamps','--since',since,'--tail','20000','yoink']);
const events = parseEvents(logs.stdout+'\n'+logs.stderr,request);
const report = {version:1,collected_at:new Date().toISOString(),container:{image:current.Image,status:current.State.Status,restarts:current.RestartCount,oom:current.State.OOMKilled,log_driver:current.HostConfig.LogConfig.Type},since,requests:summarize(events),limits:['Only the current container and last 20000 retained log lines are inspected. No request text, generated text, credentials, endpoint URLs or environment values are emitted.','Missing provider completion metadata cannot be reconstructed from old delivery records.']};
if (options['--snapshot']) {
  const selected = request ? report.requests[0] : undefined;
  if (request && !selected) throw new Error('No retained chat/message mapping for this UUID. Collect without --request or use --snapshot latest --match with a non-secret reply fragment. Older snapshots do not store diagnostic UUIDs.');
  const directory = fs.realpathSync(path.join(root,'data/native-restore'));
  let target = options['--snapshot'];
  if (target.startsWith('data/native-restore/')) target = target.slice('data/native-restore/'.length);
  if (target !== 'latest' && !/^restored-[a-zA-Z0-9._-]+\.db$/.test(target)) throw new Error('Use latest or an inactive restored-*.db basename inside data/native-restore.');
  const mounts = [[directory,'/snapshots'],[path.join(root,'src/yoink/core/db/query.py'),'/query_loader.py'],[path.join(root,'backend/scripts/insight-diagnostics'),'/queries']].flatMap(([source,destination])=>['--mount',`type=bind,source=${source},target=${destination},readonly`]);
  const result = docker(['run','-i','--rm','--pull','never','--network','none','--read-only','--cap-drop','ALL','--security-opt','no-new-privileges','--user','10001:10001','--memory','128m','--pids-limit','16',...mounts,'--entrypoint','/usr/local/bin/python',current.Image,'-I','-B','/queries/query-snapshot.py'],JSON.stringify({snapshot:target,match:options['--match'] || '',chat_id:selected?.chat_id ?? null,input_message_id:selected?.input_message_id ?? null}));
  report.snapshot = JSON.parse(result.stdout);
}
console.log(JSON.stringify(report,null,2));
