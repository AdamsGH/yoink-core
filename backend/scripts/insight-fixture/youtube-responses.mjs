import { CONTENT } from './services.mjs';
export const VIDEO_ID='IxTlsRun001';
export const VIDEO_URL=`https://www.youtube.com/watch?v=${VIDEO_ID}`;
export const TRANSCRIPT='Offline Insight transcript marker: a café telescope measures the quiet moon.';
export const TLS_RESULT='Offline TLS café completion.';
export function youtubeResponses() {
  const captions=`/api/timedtext?v=${VIDEO_ID}&lang=en`;
  const player={videoDetails:{videoId:VIDEO_ID,title:'Offline Insight telescope',lengthSeconds:'123',shortDescription:'DESCRIPTION MUST NOT REPLACE TRANSCRIPT'},captions:{playerCaptionsTracklistRenderer:{captionTracks:[{languageCode:'en',baseUrl:`https://www.youtube.com${captions}`} ]}}};
  return {[`/watch?v=${VIDEO_ID}`]:{type:'text/html',body:`<html><script>var ytInitialPlayerResponse = ${JSON.stringify(player)};</script></html>`},[captions]:{type:'text/xml',body:`<transcript><text start="0" dur="12">${TRANSCRIPT}</text></transcript>`}};
}
export function endpointConfig(){return {youtube:youtubeResponses(),marker:TRANSCRIPT,articleMarker:CONTENT,result:TLS_RESULT,model:'fixture-model',geminiModel:'fixture-gemini',byokKey:'fixture-byok-one',geminiKey:'fixture-gemini-key'};}
