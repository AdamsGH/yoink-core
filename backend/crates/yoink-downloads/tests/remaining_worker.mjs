// Deterministic downloader worker fixture. No source HTTP or credentials leave process.
import { writeFile } from 'node:fs/promises';
let input = ''; for await (const chunk of process.stdin) input += chunk;
const r = JSON.parse(input);
let descriptor;
if (r.task.media_mode === 'links' || r.task.media_mode === 'audio_links') {
  const links = {video_url: r.task.media_mode === 'links' ? 'https://cdn.example/video?signature=fixture' : null,
    audio_url: 'https://cdn.example/audio?signature=fixture'};
  const bytes = Buffer.from(JSON.stringify(links));
  await writeFile('links.json', bytes);
  descriptor = {path:'links.json', media_type:'application/json', size_bytes:bytes.length};
} else {
  const bytes = Buffer.concat([Buffer.from([0,0,0,24]),Buffer.from('ftypisom0000')]);
  await writeFile('media.mp4',bytes);
  descriptor = {path:'media.mp4',media_type:'video/mp4',size_bytes:bytes.length,
    metadata:{title:'Fixture',description:'',tags:[],categories:[]}};
  if (r.options.media_info) descriptor.media_info={size_bytes:bytes.length,streams:[{kind:'video',codec:'h264',width:1280,height:720}]};
}
process.stdout.write(JSON.stringify({version:r.version,operation_id:r.operation_id,attempt:r.attempt,
  event:{type:'completed',artifacts:[descriptor]}})+'\n');
