import { appendFile, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { spawn } from "node:child_process";
import { join } from "node:path";

export const dynamic = "force-dynamic";

type Stream = "research" | "raw_ticks";
type StreamSpec = {
  label: string;
  command: string;
  args: (root: string) => string[];
  processNeedle: string;
  log: string;
  pid: string;
  highVolume: boolean;
};

const root = join(process.cwd(), "..");
const runtimeDir = join(root, "var", "capture-control");
const streams: Record<Stream, StreamSpec> = {
  research: {
    label: "COMPACT RESEARCH SNAPSHOTS",
    command: "python3",
    args: (projectRoot) => [join(projectRoot, "scripts", "capture_snapshots.py"), "--interval", "15"],
    processNeedle: "scripts/capture_snapshots.py",
    log: join(root, "var", "log", "recorder.log"),
    pid: join(runtimeDir, "research.pid"),
    highVolume: false,
  },
  raw_ticks: {
    label: "RAW TICK JOURNALS",
    command: "sh",
    args: (projectRoot) => [join(projectRoot, "terminal", "scripts", "run-tick-capture.sh")],
    processNeedle: "run-tick-capture.sh",
    log: join(root, "var", "log", "tick-capture-control.log"),
    pid: join(runtimeDir, "raw_ticks.pid"),
    highVolume: true,
  },
};

async function commandFor(pid: number): Promise<string | null> {
  const result = await new Promise<{ code: number | null; output: string }>((resolve) => {
    const child = spawn("ps", ["-p", String(pid), "-o", "command="], { stdio: ["ignore", "pipe", "ignore"] });
    let output = "";
    child.stdout.on("data", (chunk: Buffer) => { output += chunk.toString(); });
    child.on("close", (code) => resolve({ code, output }));
    child.on("error", () => resolve({ code: 1, output: "" }));
  });
  return result.code === 0 && result.output.trim() ? result.output.trim() : null;
}

async function savedPid(path: string): Promise<number | null> {
  try {
    const raw = (await readFile(path, "utf8")).trim();
    return /^\d{1,10}$/.test(raw) ? Number(raw) : null;
  } catch { return null; }
}

async function externalPid(needle: string): Promise<number | null> {
  const output = await new Promise<string>((resolve) => {
    const child = spawn("pgrep", ["-f", needle], { stdio: ["ignore", "pipe", "ignore"] });
    let text = "";
    child.stdout.on("data", (chunk: Buffer) => { text += chunk.toString(); });
    child.on("close", () => resolve(text));
    child.on("error", () => resolve(""));
  });
  for (const raw of output.split(/\s+/)) {
    if (!/^\d{1,10}$/.test(raw)) continue;
    const pid = Number(raw);
    const command = await commandFor(pid);
    if (command?.includes(needle)) return pid;
  }
  return null;
}

async function status(stream: Stream) {
  const spec = streams[stream];
  const pid = await savedPid(spec.pid);
  if (pid !== null) {
    const command = await commandFor(pid);
    if (command?.includes(spec.processNeedle)) {
      return { stream, label: spec.label, state: "RUNNING", pid, managed: true, high_volume: spec.highVolume, log: spec.log };
    }
    await rm(spec.pid, { force: true });
  }
  const external = await externalPid(spec.processNeedle);
  if (external !== null) {
    return { stream, label: spec.label, state: "RUNNING", pid: external, managed: false, high_volume: spec.highVolume, log: spec.log };
  }
  return { stream, label: spec.label, state: "STOPPED", pid: null, managed: false, high_volume: spec.highVolume, log: spec.log };
}

async function start(stream: Stream) {
  const spec = streams[stream];
  const current = await status(stream);
  if (current.state === "RUNNING") return current;
  await mkdir(runtimeDir, { recursive: true });
  await mkdir(join(root, "var", "log"), { recursive: true });
  await appendFile(spec.log, `${new Date().toISOString()} [capture-control] explicit start ${stream}\n`);
  const child = spawn(spec.command, spec.args(root), {
    cwd: root,
    detached: true,
    stdio: ["ignore", "ignore", "ignore"],
  });
  child.unref();
  if (!child.pid) throw new Error("capture process did not return a PID");
  await writeFile(spec.pid, `${child.pid}\n`, { encoding: "utf8", flag: "wx" });
  return { stream, label: spec.label, state: "STARTING", pid: child.pid, managed: true, high_volume: spec.highVolume, log: spec.log };
}

async function stop(stream: Stream) {
  const spec = streams[stream];
  const pid = (await savedPid(spec.pid)) ?? await externalPid(spec.processNeedle);
  if (pid === null) return status(stream);
  const command = await commandFor(pid);
  if (!command?.includes(spec.processNeedle)) {
    await rm(spec.pid, { force: true });
    throw new Error("refusing to signal an unverified capture process");
  }
  // Each started capture is detached into its own process group.  Terminating
  // the group lets the tick-capture wrapper close both journals cleanly.
  try { process.kill(-pid, "SIGTERM"); } catch { process.kill(pid, "SIGTERM"); }
  await appendFile(spec.log, `${new Date().toISOString()} [capture-control] explicit stop ${stream} pid=${pid}\n`);
  await rm(spec.pid, { force: true });
  return { stream, label: spec.label, state: "STOPPING", pid, managed: true, high_volume: spec.highVolume, log: spec.log };
}

export async function GET() {
  return Response.json({
    paper_only: true,
    streams: await Promise.all((["research", "raw_ticks"] as const).map(status)),
    rule: "Explicit local controls only. Stop preserves every existing journal; no data is deleted.",
  });
}

export async function POST(request: Request) {
  let body: { stream?: unknown; action?: unknown };
  try { body = await request.json() as { stream?: unknown; action?: unknown }; }
  catch { return Response.json({ error: "invalid JSON" }, { status: 400 }); }
  if ((body.stream !== "research" && body.stream !== "raw_ticks") || (body.action !== "start" && body.action !== "stop")) {
    return Response.json({ error: "stream/action invalid" }, { status: 400 });
  }
  try {
    const result = body.action === "start" ? await start(body.stream) : await stop(body.stream);
    return Response.json({ paper_only: true, result, streams: await Promise.all((["research", "raw_ticks"] as const).map(status)) });
  } catch (error) {
    return Response.json({ error: error instanceof Error ? error.message : "capture control failed" }, { status: 409 });
  }
}
