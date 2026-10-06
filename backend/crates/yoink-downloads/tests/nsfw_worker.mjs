// Disposable executor contract fixture: no network, no production data.
import { writeFile } from 'node:fs/promises';
let input = '';
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
const payload = Buffer.concat([Buffer.from([0, 0, 0, 24]), Buffer.from('ftypisom0000')]);
await writeFile('media.mp4', payload);
process.stdout.write(JSON.stringify({ version: request.version, operation_id: request.operation_id,
  attempt: request.attempt, event: {type: 'completed', artifacts: [{path: 'media.mp4',
    media_type: 'video/mp4', size_bytes: payload.length, metadata: {
      title: 'Fixture sensitivecontent sample', description: '', tags: [], categories: [],
    }}]}}) + '\n');
