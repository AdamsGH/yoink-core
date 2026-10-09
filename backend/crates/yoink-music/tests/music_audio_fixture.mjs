import { promises as fs } from "node:fs";
let input = "";
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
const bytes = Buffer.from("ID3fixtureaudio");
await fs.writeFile("song.mp3", bytes);
process.stdout.write(JSON.stringify({
  version: request.version,
  operation_id: request.operation_id,
  attempt: request.attempt,
  event: { type: "completed", artifacts: [{ path: "song.mp3", size_bytes: bytes.length, media_type: "audio/mpeg" }] }
}) + "\n");
