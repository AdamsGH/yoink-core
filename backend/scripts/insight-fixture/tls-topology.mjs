import assert from 'node:assert/strict';
import { randomBytes, randomInt } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { endpointConfig } from './youtube-responses.mjs';
export const TLS_HOSTS=['byok-success.fixture.invalid','www.youtube.com','generativelanguage.googleapis.com'];
export async function dockerIpv4Subnets(docker){
 const networks=(await docker(['network','ls','-q'])).split('\n').filter(Boolean);
 if(!networks.length)return [];
 // Full network metadata can exceed the process runner's bounded output.
 const configs=await docker(['network','inspect','--format','{{json .IPAM.Config}}',...networks]);
 return configs.split('\n').filter(Boolean).flatMap(line=>JSON.parse(line)??[]).map(config=>config.Subnet).filter(subnet=>subnet&&!subnet.includes(':'));
}
function range(cidr){const [ip,prefix]=cidr.split('/');assert.match(ip,/^\d+\.\d+\.\d+\.\d+$/);const octets=ip.split('.').map(Number);assert(octets.every(n=>n<=255));const bits=Number(prefix);assert(Number.isInteger(bits)&&bits>=0&&bits<=32);const n=octets.reduce((a,b)=>a*256+b,0),size=2**(32-bits);return [Math.floor(n/size)*size,Math.floor(n/size)*size+size-1];}
export function overlaps(a,b){const [lo,hi]=range(a),[l,h]=range(b);return lo<=h&&l<=hi;}
export function occupiedSubnets(hostRoutes,dockerSubnets){
 assert(Array.isArray(hostRoutes)&&hostRoutes.length,'complete host route snapshot required');
 assert(Array.isArray(dockerSubnets),'Docker IPAM snapshot required');
 return [...hostRoutes.filter(route=>!(route.gateway && route.table!==undefined && !['main','local','default','254','255','253'].includes(String(route.table)))).map(route=>{
  assert.equal(typeof route.dst,'string','IPv4 route destination required');
  return route.dst==='default'?'0.0.0.0/0':route.dst.includes('/')?route.dst:`${route.dst}/32`;
 }),...dockerSubnets];
}
export function chooseSubnets(routes){assert(Array.isArray(routes)&&routes.length,'complete host + Docker route snapshot required');routes.forEach(range);for(let i=0;i<128;i++){const data=`93.184.${randomInt(1,255)}.0/24`,control=`172.${randomInt(20,30)}.${randomInt(1,255)}.0/24`; // Default routes do not reserve every address; all other routes do.
 const reserved=routes.filter(r=>!r.endsWith('/0'));if(!reserved.some(r=>overlaps(r,data)||overlaps(r,control)))return {data,control};}throw Error('no non-overlapping fixture routes');}
// All execution is coordinator-owned. routes() must include host routes AND all
// Docker IPAM subnets. defer is invoked before any resource allocation.
export async function createTlsTopology({docker,helperContainer,startContainer,defer,routes,image,waitFor}){
 for(const fn of [docker,helperContainer,startContainer,defer,routes,waitFor])assert.equal(typeof fn,'function');
 assert.match(image,/^(sha256:[a-f0-9]{64}|[A-Za-z0-9][A-Za-z0-9._:/-]*@sha256:[a-f0-9]{64})$/,'immutable existing helper image');
 const subnets=chooseSubnets(await routes());
 const id=randomBytes(8).toString('hex'),dataNetwork=`yoink-insight-tls-data-${id}`,controlNetwork=`yoink-insight-tls-control-${id}`,trustVolume=`yoink-insight-tls-trust-${id}`,secretVolume=`yoink-insight-tls-secret-${id}`;
 const containers=[],networks=[],volumes=[];let closing;
 const close=()=>closing??=(async()=>{const errors=[];for(const [kind,names] of [['container',containers],['network',networks],['volume',volumes]])for(const name of [...names].reverse())try{const list=await docker(kind==='container'?['ps','-aq','--filter',`name=^/${name}$`]:[kind,'ls','--format','{{.Name}}']);if(kind==='container'?list:list.split('\n').includes(name))await docker([kind==='container'?'rm':kind,...(kind==='container'?['--force']:['rm']),name]);}catch(e){errors.push(e);}if(errors.length)throw new AggregateError(errors,'TLS topology cleanup');})();
 defer(close);
 const dataIP=subnets.data.replace('0/24','4'),dnsIP=subnets.control.replace('0/24','3'),controlIP=subnets.control.replace('0/24','4');
 try{
 for(const [name,subnet] of [[dataNetwork,subnets.data],[controlNetwork,subnets.control]]){networks.push(name);await docker(['network','create','--internal','--subnet',subnet,name]);const inspect=JSON.parse(await docker(['network','inspect',name]))[0];assert.equal(inspect.Internal,true);assert.equal(inspect.IPAM.Config[0].Subnet,subnet);}
 for(const name of [trustVolume,secretVolume]){volumes.push(name);await docker(['volume','create',name]);}
 const dns=await readFile(new URL('./fixture-dns.py',import.meta.url),'utf8'),tls=await readFile(new URL('./offline-tls.py',import.meta.url),'utf8');
 const bootstrap=`import json,sys,pathlib,subprocess,ssl\np=json.load(sys.stdin)\ns=pathlib.Path('/secrets'); t=pathlib.Path('/trust')\n(s/'dns.py').write_text(p['dns']); (s/'tls.py').write_text(p['tls'])\ndef run(*a): subprocess.run(['openssl',*a],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)\nrun('req','-x509','-newkey','rsa:2048','-nodes','-days','2','-subj','/CN=Owned Insight fixture CA','-addext','basicConstraints=critical,CA:TRUE','-keyout',str(s/'ca.key'),'-out',str(s/'ca.pem'))\nfor host in p['hosts']:\n run('req','-newkey','rsa:2048','-nodes','-subj','/CN='+host,'-keyout',str(s/(host+'.key')),'-out',str(s/(host+'.csr')))\n ext=s/'extensions'; ext.write_text('subjectAltName=DNS:'+host+'\\nbasicConstraints=critical,CA:FALSE\\nextendedKeyUsage=serverAuth\\n')\n run('x509','-req','-in',str(s/(host+'.csr')),'-CA',str(s/'ca.pem'),'-CAkey',str(s/'ca.key'),'-CAcreateserial','-days','2','-extfile',str(ext),'-out',str(s/(host+'.pem')))\n run('verify','-CAfile',str(s/'ca.pem'),'-verify_hostname',host,str(s/(host+'.pem')))\nordinary=ssl.get_default_verify_paths().cafile\nassert ordinary\n(t/'bundle.pem').write_bytes(pathlib.Path(ordinary).read_bytes()+b'\\n'+(s/'ca.pem').read_bytes())\n(t/'fixture-ca.pem').write_bytes((s/'ca.pem').read_bytes())\nrun('x509','-in',str(s/'ca.pem'),'-outform','DER','-out',str(t/'fixture-ca.der'))\nprint(str(pathlib.Path(ordinary).resolve()))`;
 const mounts=['--mount',`type=volume,source=${secretVolume},target=/secrets`,'--mount',`type=volume,source=${trustVolume},target=/trust`];
 const bundlePath=await helperContainer(['--network','none','--user','0',...mounts,'--entrypoint','/usr/local/bin/python',image,'-I','-B','-c',bootstrap+"\nimport os\nfor host in p['hosts']:\n key=s/(host+'.key'); os.chown(key,10001,10001); key.chmod(0o400)"],30000,JSON.stringify({dns,tls,hosts:TLS_HOSTS}));assert.match(bundlePath,/^\/[\w/.-]+$/);
 const hardening=['--read-only','--cap-drop','ALL','--security-opt','no-new-privileges','--pids-limit','32','--memory','96m','--mount',`type=volume,source=${secretVolume},target=/secrets,readonly`];
 const dnsName=`yoink-insight-tls-dns-${id}`;containers.push(dnsName);await startContainer(['--name',dnsName,'--network',controlNetwork,'--ip',dnsIP,...hardening,'--entrypoint','/usr/local/bin/python',image,'-I','-B','/secrets/dns.py',dnsIP,JSON.stringify(Object.fromEntries(TLS_HOSTS.map(h=>[h,dataIP])))]);
 const tlsName=`yoink-insight-tls-server-${id}`;containers.push(tlsName);const server=await startContainer(['--name',tlsName,'--network',dataNetwork,'--ip',dataIP,'--dns',dnsIP,...hardening,'--entrypoint','/usr/local/bin/python',image,'-I','-B','/secrets/tls.py',dataIP,controlIP,JSON.stringify(endpointConfig())]);
 // TLS process must not bind until its control interface exists: coordinator's
 // start callback may start immediately, so connect before control bind retry.
 await docker(['network','connect','--ip',controlIP,controlNetwork,server]);
 const state=async()=>JSON.parse(await helperContainer(['--network',controlNetwork,'--entrypoint','/usr/local/bin/python',image,'-I','-B','-c','import urllib.request,sys; print(urllib.request.build_opener(urllib.request.ProxyHandler({})).open(sys.argv[1],timeout=2).read().decode())',`http://${controlIP}:8080/state`]));
 await waitFor(async()=>{try{return Array.isArray((await state()).events);}catch{return false;}},'owned TLS endpoints',10000);
 const verifyTlsRejections=async()=>{
 const probe=`import socket,ssl,sys\nip=sys.argv[1]\ndef connect(ctx,name):\n return ctx.wrap_socket(socket.create_connection((ip,443),2),server_hostname=name)\ntrusted=ssl.create_default_context(cafile='/secrets/ca.pem')\nwith connect(trusted,'byok-success.fixture.invalid') as s:\n s.sendall(b'GET /article HTTP/1.1\\r\\nHost: byok-success.fixture.invalid:443\\r\\nConnection: close\\r\\n\\r\\n'); assert b' 200 ' in s.recv(4096)\nwith connect(trusted,'byok-success.fixture.invalid') as s:\n s.sendall(b'GET /v1/models HTTP/1.1\\r\\nHost: www.youtube.com\\r\\nConnection: close\\r\\n\\r\\n'); assert b' 403 ' in s.recv(4096)\nfor ctx,name in [(ssl.create_default_context(),'byok-success.fixture.invalid'),(trusted,'unknown.fixture.invalid')]:\n try:\n  s=connect(ctx,name); s.close()\n except ssl.SSLError: pass\n else: raise AssertionError('TLS rejection missing')\nprint('TLS confinement verified')`;
 assert.equal(await docker(['exec',server,'/usr/local/bin/python','-I','-B','-c',probe,dataIP]),'TLS confinement verified');
 };
 return {dataNetwork,controlNetwork,networkContainer:server,dnsAddress:dnsIP,dataAddress:dataIP,state,close,verifyTlsRejections,trustVolume,bundlePath,logs:()=>docker(['logs',tlsName]),
  bundleMount: ['--mount',`type=volume,source=${trustVolume},target=/fixture-trust,readonly`,'--mount',`type=volume,source=${trustVolume},volume-subpath=bundle.pem,target=${bundlePath},readonly`],
  env:{YOINK_RUST_INSIGHT_GEMINI_KEY:'fixture-gemini-key',YOINK_RUST_INSIGHT_GEMINI_MODEL:'fixture-gemini'},nativeTrustFile:'/fixture-trust/fixture-ca.der',pythonBundleFile:'/fixture-trust/bundle.pem'};
 }catch(error){const logs=[];for(const name of containers)try{logs.push(`${name}: ${await docker(['logs',name])}`);}catch{}const failure=new Error(`TLS topology failed\n${logs.join('\n')}`,{cause:error});try{await close();}catch(cleanup){throw new AggregateError([failure,cleanup],'TLS allocation/cleanup');}throw failure;}
}
