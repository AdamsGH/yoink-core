import { promises as fs } from "node:fs";
import { dirname } from "node:path";
import { spawn } from "node:child_process";
import { once } from "node:events";

const mode = process.argv[2] ?? "normal";
if (mode === "stdin-hang") setInterval(() => {}, 1000);
let input = "";
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input.trim());
const emit = (event, overrides = {}) =>
  process.stdout.write(JSON.stringify({
    version: 1,
    operation_id: request.operation_id,
    attempt: request.attempt,
    event,
    ...overrides,
  }) + "\n");
const payload = Buffer.concat([Buffer.from([0, 0, 0, 24]), Buffer.from("ftyp"), Buffer.from("isom0000")]);
const writeArtifact = async (path = "media.mp4", bytes = payload) => {
  await fs.mkdir(dirname(path), { recursive: true });
  await fs.writeFile(path, bytes);
  return { path, size_bytes: bytes.length, media_type: "video/mp4" };
};
const completed = (artifacts) => ({ type: "completed", artifacts });
const artifactEvent = async (path) => completed([await writeArtifact(path)]);
const spawnMarkerWriter = (marker) => spawn(process.execPath, ["-e", `
  const fs = require("node:fs");
  const marker = process.argv[1];
  fs.writeFileSync(marker, "ready");
  setInterval(() => fs.appendFileSync(marker, "x"), 10);
`, marker], { detached: false, stdio: "ignore" });

if (mode === "hang" || mode === "stdout-partial-hang" || mode === "stdin-hang" || mode === "cancel" || mode === "timeout-descendant") {
  if (mode === "stdout-partial-hang") process.stdout.write('{"version":');
  if (mode === "stdin-hang") {
    process.stdin.pause();
    await new Promise(() => {});
  }
  if ((mode === "timeout-descendant" || mode === "cancel") && process.argv[3]) spawnMarkerWriter(process.argv[3]);
  setInterval(() => {}, 1000);
} else if (mode === "noexit") {
  process.exit(0);
} else if (mode === "stderr-overflow") {
  process.stderr.write("x".repeat(5 * 1024 * 1024));
} else if (mode === "oversize" || mode === "stdout-overflow") {
  process.stdout.write("x".repeat(5 * 1024 * 1024));
} else if (mode === "bad-json") {
  process.stdout.write("{broken}\n");
} else if (mode === "version" || mode === "attempt" || mode === "path" || mode === "identity") {
  const a = await artifactEvent("media.mp4");
  emit(a, mode === "version" ? { version: 99 } : mode === "attempt" ? { attempt: request.attempt + 1 } : { operation_id: "not-the-request" });
} else if (mode === "missing-artifact") {
  emit(completed([{ path: "missing.mp4", size_bytes: payload.length, media_type: "video/mp4" }]));
} else if (mode === "symlink" || mode === "parent-symlink" || mode === "traversal") {
  if (mode === "parent-symlink") {
    await fs.mkdir("real");
    await fs.writeFile("real/media.mp4", payload);
    await fs.symlink("real", "linked");
    emit(completed([{ path: "linked/media.mp4", size_bytes: payload.length, media_type: "video/mp4" }]));
  } else if (mode === "traversal") {
    emit(completed([{ path: "../media.mp4", size_bytes: payload.length, media_type: "video/mp4" }]));
  } else {
    await fs.symlink("/etc/passwd", "media.mp4");
    emit(completed([{ path: "media.mp4", size_bytes: 128, media_type: "video/mp4" }]));
  }
} else if (mode === "hardlink") {
  await fs.writeFile("original.mp4", payload);
  await fs.link("original.mp4", "media.mp4");
  emit(completed([{ path: "media.mp4", size_bytes: payload.length, media_type: "video/mp4" }]));
} else if (mode === "magic-mismatch" || mode === "size-mismatch") {
  const bytes = mode === "magic-mismatch" ? Buffer.from("not an mp4") : payload;
  await fs.writeFile("media.mp4", bytes);
  emit(completed([{ path: "media.mp4", size_bytes: mode === "size-mismatch" ? payload.length + 1 : bytes.length, media_type: "video/mp4" }]));
} else if (mode === "workspace-filecount") {
  for (let i = 0; i < 40; i++) await fs.writeFile(`file-${i}`, "x");
  emit(await artifactEvent("media.mp4"));
} else if (mode === "workspace-bytecap") {
  await fs.writeFile("large", Buffer.alloc(2 * 1024 * 1024));
  emit(await artifactEvent("media.mp4"));
} else if (mode === "fifo") {
  const mkfifo = spawn("mkfifo", ["media.mp4"]);
  await once(mkfifo, "exit");
  emit(completed([{ path: "media.mp4", size_bytes: payload.length, media_type: "video/mp4" }]));
} else if (mode === "nonzero") {
  await artifactEvent("media.mp4").then(emit);
  process.exitCode = 7;
} else if (mode === "multiple-terminal") {
  const a = await artifactEvent("media.mp4");
  emit(a);
  emit(a);
} else if (mode === "eof-before-terminal") {
  process.exit(0);
} else if (mode === "stdout-partial") {
  process.stdout.write('{"version":');
} else if (mode === "stdin-stall") {
  await new Promise(() => {});
} else if (mode === "progress") {
  emit({ type: "progress", percent: 50, stage: "processing" });
  emit(await artifactEvent("media.mp4"));
} else if (mode === "descendant") {
  const descendant = spawnMarkerWriter(process.argv[3]);
  const deadline = Date.now() + 2000;
  while (true) {
    try { await fs.access(process.argv[3]); break; } catch {}
    if (Date.now() >= deadline) throw new Error("fixture descendant did not start");
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  await fs.writeFile(process.argv[4], String(process.pid));
  emit(await artifactEvent("media.mp4"));
  descendant.unref();
} else {
  emit(await artifactEvent("media.mp4"));
}
