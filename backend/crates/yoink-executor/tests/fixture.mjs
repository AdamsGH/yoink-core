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
    version: 2,
    operation_id: request.operation_id,
    attempt: request.attempt,
    event,
    ...overrides,
  }) + "\n");
const payload = Buffer.concat([Buffer.from([0, 0, 0, 24]), Buffer.from("ftyp"), Buffer.from("isom0000")]);
if (mode === "environment") {
  if (((await fs.stat(process.cwd())).mode & 0o7777) !== 0o700) throw new Error("workspace must be owner-only");
  const expected = {
    YOINK_WORKER_MAX_BYTES: "1048576",
    YOINK_WORKER_MAX_ITEMS: "100",
    YOINK_WORKER_TOTAL_MAX_BYTES: "2097152",
    YOINK_WORKER_ALLOWED_PROXY_ORIGINS: "[\"https://proxy.example:8443\"]",
    YOINK_WORKER_ALLOWED_PRIVATE_ORIGINS: "[\"http://127.0.0.1:8080\"]",
    YOINK_WORKER_SOCKET_TIMEOUT_SECS: "2",
    YOINK_WORKER_PROCESSING_TIMEOUT_SECS: "3",
  };
  for (const [key, value] of Object.entries(expected)) {
    if (process.env[key] !== value) throw new Error(`unexpected ${key}`);
  }
  for (const key of ["MAX_ITEMS", "TOTAL_MAX_BYTES", "ALLOWED_PROXY_ORIGINS"]) {
    if (Object.hasOwn(process.env, key)) throw new Error(`legacy key ${key}`);
  }
}
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

if (mode.startsWith("music-")) {
  if (mode === "music-artifact") await writeArtifact("unexpected.mp4");
  if (mode === "music-audio") {
    const bytes = Buffer.from("ID3fixtureaudio");
    await fs.writeFile("song.mp3", bytes);
    emit(completed([{path:"song.mp3",size_bytes:bytes.length,media_type:"audio/mpeg"}]));
  } else emit({type:"music_completed",result:{kind:"metadata",metadata:{title:"Song",artist:"Artist",thumbnail_url:null}}});
} else if (mode.startsWith("content-")) {
  if (mode === 'content-denied') emit({type:'content_denied'});
  else if (mode === 'content-artifact') { await writeArtifact('unexpected.mp4'); emit({type:'content_completed',result:{text:'body',title:null}}); }
  else emit({type:'content_completed',result:{text: mode === 'content-bound' ? 'x'.repeat(131073) : 'body',title:'Title'}});
} else if (mode.startsWith("search-")) {
  if (request.task.kind !== 'youtube_search' || Object.hasOwn(request,'source_url') || Object.hasOwn(request,'options') || request.network?.cookie_jar) throw new Error('invalid search envelope');
  const entry = {url:'https://www.youtube.com/watch?v=fixture0000',title:'Fixture',channel:'Channel',duration_seconds:1,view_count:1};
  if (mode === 'search-artifact') await writeArtifact('unexpected.mp4');
  if (mode === 'search-overflow') process.stdout.write('x'.repeat(33000));
  else if (mode === 'search-denial') emit({type:'failure',class:'source_unavailable'});
  else if (mode === 'search-exit') process.exitCode = 7;
  else {
    if (mode === 'search-url') entry.url = 'https://evil.example/secret';
    emit({type:'search_completed',entries: mode === 'search-zero' ? [] : mode === 'search-duplicate' ? [entry,entry] : [entry]});
  }
} else if (mode === "hang" || mode === "stdout-partial-hang" || mode === "stdin-hang" || mode === "cancel" || mode === "timeout-descendant") {
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
  for (let i = 0; i < 513; i++) await fs.writeFile(`file-${i}`, "x");
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
} else if (mode === "identity-progress") {
  emit({ type: "progress", percent: 25, stage: "fetching" }, { operation_id: "not-the-request" });
  emit(await artifactEvent("media.mp4"));
} else if (["many", "duplicate", "aggregate-overlimit", "mixed-types"].includes(mode)) {
  const descriptors = [];
  if (mode === "many") {
    for (let i = 0; i < 101; i++) descriptors.push(await writeArtifact(`item-${i}.mp4`));
  } else if (mode === "duplicate") {
    const descriptor = await writeArtifact("media.mp4");
    descriptors.push(descriptor, descriptor);
  } else if (mode === "aggregate-overlimit") {
    for (let i = 0; i < 3; i++) {
      const path = `part-${i}.mp4`;
      await fs.writeFile(path, Buffer.concat([payload, Buffer.alloc(900 * 1024)]));
      descriptors.push({ path, size_bytes: 900 * 1024 + payload.length, media_type: "video/mp4" });
    }
  } else {
    const items = [
      ["sound.mp3", Buffer.from("ID3\x04\x00\x00"), "audio/mpeg"],
      ["picture.png", Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), "image/png"],
      ["archive.zip", Buffer.from("PK\x03\x04"), "application/zip"],
    ];
    for (const [path, bytes, media_type] of items) {
      await fs.writeFile(path, bytes);
      descriptors.push({ path, size_bytes: bytes.length, media_type });
    }
  }
  emit(completed(descriptors));
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
