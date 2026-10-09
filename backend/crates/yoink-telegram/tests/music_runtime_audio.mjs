import { promises as fs } from "node:fs";
import { spawn } from "node:child_process";
let input = "";
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
if (process.argv[2] === "hang") {
  const marker = process.argv[3];
  spawn(process.execPath, ["-e", "require('node:fs').writeFileSync(process.argv[1],String(process.pid));setInterval(()=>{},1000)", marker], { stdio: "ignore" });
  await fs.writeFile(marker + ".parent", String(process.pid));
  await new Promise(() => setInterval(() => {}, 1000));
}
const bytes = Buffer.from("ID3fixtureaudio");
await fs.writeFile("song.mp3", bytes);
process.stdout.write(JSON.stringify({
  version: request.version,
  operation_id: request.operation_id,
  attempt: request.attempt,
  event: { type: "completed", artifacts: [{ path: "song.mp3", size_bytes: bytes.length, media_type: "audio/mpeg" }] }
}) + "\n");
