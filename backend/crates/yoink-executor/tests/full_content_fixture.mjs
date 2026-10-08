import fs from 'node:fs';
import {spawn} from 'node:child_process';
let input=''; for await(const chunk of process.stdin) input+=chunk;
const r=JSON.parse(input), mode=process.argv[2];
const emit=(event, extra={})=>process.stdout.write(JSON.stringify({version:2, operation_id:r.operation_id, attempt:r.attempt,event,...extra})+'\n');
const result={text:'😀"\\ real',title:'Title',video_id:null,video_seconds:null,via:'html-main'};
if(mode==='hang') {
  const marker=process.argv[3];
  const descendant=spawn(process.execPath,['-e',`const fs=require('node:fs');fs.writeFileSync(process.argv[1],String(process.pid));setInterval(()=>fs.appendFileSync(process.argv[1]+'.ticks','x'),10);`,marker],{stdio:'ignore'});
  fs.writeFileSync(marker+'.parent',String(process.pid));
  setInterval(()=>{},1000);
} else if(mode==='artifact') {fs.writeFileSync('unexpected','x');emit({type:'full_content_completed',result});}
else if(mode==='wrong-id') emit({type:'full_content_completed',result},{operation_id:'12345678-1234-1234-1234-123456789abc'});
else if(mode==='v1') emit({type:'full_content_completed',result},{version:1});
else if(mode==='duplicate') {emit({type:'full_content_completed',result});emit({type:'full_content_completed',result});}
else if(mode==='progress') {emit({type:'progress',percent:1,stage:'fetching'});emit({type:'full_content_completed',result});}
else if(mode==='cap') {result.text='x'.repeat(101);emit({type:'full_content_completed',result});}
else if(mode==='video') {result.video_id='abcdefghijk';emit({type:'full_content_completed',result});}
else if(mode==='error') emit({type:'full_content_failed',error:'content_too_large'});
else emit({type:'full_content_completed',result});
