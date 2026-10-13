import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { chooseSubnets, overlaps, occupiedSubnets, createTlsTopology, TLS_HOSTS, dockerIpv4Subnets } from './tls-topology.mjs';
import { endpointConfig, VIDEO_ID, TRANSCRIPT } from './youtube-responses.mjs';
test('Docker inventory requests only IPAM within the bounded process output',async()=>{
 const docker=async args=>{
  if(args[1]==='ls')return 'first\nsecond\nthird\nfourth\n';
  const metadata=JSON.stringify({Containers:'x'.repeat(30000),IPAM:{Config:[{Subnet:'10.0.0.0/8'}]}});
  if(!args.includes('--format'))return metadata.slice(-24*1024);
  assert.deepEqual(args,['network','inspect','--format','{{json .IPAM.Config}}','first','second','third','fourth']);
  return '[{"Subnet":"10.0.0.0/8"},{"Subnet":"fd00::/64"}]\n[]\n[{"Subnet":"172.20.0.0/16"}]\nnull\n';
 };
 assert.deepEqual(await dockerIpv4Subnets(docker),['10.0.0.0/8','172.20.0.0/16']);
 assert.deepEqual(await dockerIpv4Subnets(async()=>''),[]);
});
test('route overlap includes supernets, default route does not reserve all addresses',()=>{
 assert(overlaps('93.184.1.0/24','93.184.0.0/16'));assert(!overlaps('93.184.1.0/24','93.184.2.0/24'));
 const plan=chooseSubnets(['0.0.0.0/0','172.17.0.0/16','10.0.0.0/8']);assert(!overlaps(plan.control,'172.17.0.0/16'));
 assert.throws(()=>chooseSubnets(['93.184.0.0/16','172.16.0.0/12']),/no non-overlapping/);
 assert.throws(()=>chooseSubnets([]),/snapshot/);
});
test('only policy gateway routes are excluded from address occupancy',()=>{
 const host=[{dst:'default',gateway:'10.0.0.1'},
  {dst:'93.184.0.0/16',table:'2022',gateway:'198.18.0.2'},
  {dst:'172.16.0.0/12',table:2022,gateway:'198.18.0.2'},
  {dst:'10.0.0.0/8',table:'2022',dev:'eth0'},
  {dst:'192.168.0.0/16',table:'main',gateway:'10.0.0.2'},
  {dst:'192.0.2.0/24',table:254,gateway:'10.0.0.3'},
  {dst:'127.0.0.1',table:'local'}];
 const before=structuredClone(host),docker=['93.184.1.0/24','172.20.1.0/24'];
 const reserved=occupiedSubnets(host,docker);
 assert.deepEqual(reserved,['0.0.0.0/0','10.0.0.0/8','192.168.0.0/16','192.0.2.0/24','127.0.0.1/32',...docker]);
 assert.deepEqual(host,before,'complete snapshot is not modified');
 const plan=chooseSubnets(reserved);
 assert(docker.every(subnet=>!overlaps(subnet,plan.data)&&!overlaps(subnet,plan.control)));
 assert.throws(()=>occupiedSubnets([],docker),/snapshot/);
});
test('canned watch URL and captions encode actual extractor protocol and fixed model',()=>{
 const config=endpointConfig(),watch=config.youtube[`/watch?v=${VIDEO_ID}`].body;
 const player=JSON.parse(watch.match(/ytInitialPlayerResponse = (.*);<\/script>/)[1]);
 assert.equal(VIDEO_ID.length,11);const caption=player.captions.playerCaptionsTracklistRenderer.captionTracks[0];
 const url=new URL(caption.baseUrl);assert.equal(url.hostname,'www.youtube.com');assert(config.youtube[url.pathname+url.search].body.includes(TRANSCRIPT));assert(!player.videoDetails.shortDescription.includes(TRANSCRIPT));
 assert.equal(config.openrouterKey,'fixture-openrouter-key');assert.equal(new Set(TLS_HOSTS).size,3);
});
test('cleanup registered before allocation and all removal failures aggregated',async()=>{
 const calls=[],actions=[];let allocated=false;
 const docker=async args=>{calls.push(args);if(args[0]==='network'&&args[1]==='create'){assert.equal(actions.length,1);allocated=true;throw Error('allocation failed after creation');}if(args[1]==='ls')return calls.find(a=>a[1]==='create')?.at(-1)??'';if(args[1]==='rm')throw Error('removal failed');return '';};
 await assert.rejects(createTlsTopology({docker,helperContainer:async()=>'',startContainer:async()=>'',defer:a=>actions.push(a),routes:async()=>['0.0.0.0/0'],image:'sha256:'+ 'a'.repeat(64),waitFor:async()=>{}}),error=>error instanceof AggregateError&&error.errors.length===2);
 assert(allocated);assert(calls.some(a=>a[1]==='rm'));
});
test('overlap rejection happens before registration/allocation',async()=>{
 let calls=0;await assert.rejects(createTlsTopology({docker:async()=>calls++,helperContainer:async()=>{},startContainer:async()=>{},defer:()=>calls++,routes:async()=>['93.184.0.0/16','172.16.0.0/12'],image:'sha256:'+'a'.repeat(64),waitFor:async()=>{}}));assert.equal(calls,0);
});
test('owned Python protocols have no upstream/forwarding primitives',async()=>{
 for(const file of ['fixture-dns.py','offline-tls.py']){const source=await readFile(new URL(file,import.meta.url),'utf8');assert(!/urllib|requests|create_connection|getaddrinfo|CERT_NONE|check_hostname\s*=\s*False/.test(source));}
});
test('container-only bootstrap retains PEM bundle and exports DER native roots',async()=>{
 const networks=new Map();let bootstrap;
 const topology=await createTlsTopology({
  image:'sha256:'+'a'.repeat(64),routes:async()=>['0.0.0.0/0'],defer:()=>{},
  docker:async args=>{
   if(args[0]==='network'&&args[1]==='create')networks.set(args.at(-1),args[args.indexOf('--subnet')+1]);
   if(args[0]==='network'&&args[1]==='inspect')return JSON.stringify([{Internal:true,IPAM:{Config:[{Subnet:networks.get(args[2])}]}}]);
   return '';
  },
  startContainer:async()=> 'owned-server',
  helperContainer:async args=>{
   if(args.includes('--network')&&args[args.indexOf('--network')+1]==='none'){
    bootstrap=args[args.indexOf('-c')+1];return '/etc/ssl/certs/ca-certificates.crt';
   }
   return JSON.stringify({events:[]});
  },
  waitFor:async fn=>assert(await fn()),
 });
 assert(bootstrap.includes("run('x509','-in',str(s/'ca.pem'),'-outform','DER','-out',str(t/'fixture-ca.der'))"));
 assert(bootstrap.includes("(t/'fixture-ca.pem').write_bytes((s/'ca.pem').read_bytes())"));
 assert(bootstrap.includes("(t/'bundle.pem').write_bytes(pathlib.Path(ordinary).read_bytes()"));
 assert(bootstrap.includes('os.chown(key,10001,10001); key.chmod(0o400)'));
 assert.equal(topology.nativeTrustFile,'/fixture-trust/fixture-ca.der');
 assert.equal(topology.networkContainer,'owned-server');
 assert.equal(topology.pythonBundleFile,'/fixture-trust/bundle.pem');
 assert(topology.bundleMount.some(arg=>arg.includes('target=/fixture-trust,readonly')));
 await topology.close();
});
