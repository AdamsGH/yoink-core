// Disposable read-only-root PostgreSQL oracle; no mounted DB, ports or network.
import {spawnSync} from 'node:child_process';
import {readFileSync} from 'node:fs';
const name=`yoink-inbox-ilike-${process.pid}`;
function docker(args,input){const r=spawnSync('docker',args,{input,encoding:'utf8',timeout:30000});if(r.status!==0)throw new Error(r.stderr||String(r.error));return r.stdout.trim();}
const cases=JSON.parse(readFileSync('tests/fixtures/ilike.json','utf8'));
const quote=s=>`'${s.replaceAll("'","''")}'`;
try{
  docker(['run','--rm','--pull','never','--detach','--name',name,'--network','none','--read-only','--memory','256m','--pids-limit','64','--tmpfs','/var/lib/postgresql/data:rw,size=128m','--tmpfs','/var/run/postgresql:rw,size=16m','--tmpfs','/tmp:rw,size=16m','-e','POSTGRES_HOST_AUTH_METHOD=trust','-e','POSTGRES_INITDB_ARGS=--locale=C.UTF-8','postgres:16-alpine']);
  let ready=false;for(let i=0;i<30;i++){const r=spawnSync('docker',['exec',name,'pg_isready','-h','127.0.0.1','-U','postgres'],{encoding:'utf8'});if(r.status===0){ready=true;break}Atomics.wait(new Int32Array(new SharedArrayBuffer(4)),0,0,200);}
  if(!ready)throw new Error('disposable PostgreSQL did not become ready');
  for(const [search,value,expected] of cases){const result=docker(['exec','-i',name,'psql','-h','127.0.0.1','-X','-A','-t','-U','postgres','-v','ON_ERROR_STOP=1'],`SELECT ${quote(value)} ILIKE ${quote('%'+search+'%')};\n`);if(result!==(expected?'t':'f'))throw new Error(`ILIKE mismatch ${JSON.stringify({search,value,expected,result})}`);}
  console.log(`PostgreSQL 16 C.UTF-8 ILIKE: ${cases.length} deterministic cases passed.`);
}finally{spawnSync('docker',['rm','-f',name],{encoding:'utf8'});}
