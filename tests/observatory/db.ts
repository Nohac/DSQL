import { SQL } from "bun";
import { writeFile } from "node:fs/promises";
import { seed, type Profile } from "./seed";

const stateFile = new URL(".db-state.json", import.meta.url);
const schemaFile = new URL("schema.sql", import.meta.url).pathname;
const image = process.env.DSQL_POSTGRES_IMAGE ?? "docker.io/library/postgres:17.5-alpine";

type State = { container: string; url: string; profile: Profile };

async function podman(...args: string[]): Promise<string> {
  const process = Bun.spawn(["podman", ...args], { stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(process.stdout).text(),
    new Response(process.stderr).text(),
    process.exited,
  ]);
  if (exitCode !== 0) throw new Error(stderr.trim() || `podman ${args[0]} failed`);
  return stdout.trim();
}

async function readState(): Promise<State | undefined> {
  const file = Bun.file(stateFile);
  return (await file.exists()) ? await file.json() : undefined;
}

async function waitReady(url: string): Promise<SQL> {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    const sql = new SQL({ url, max: 1 });
    try {
      await sql`SELECT 1`;
      return sql;
    } catch {
      await sql.close();
      await Bun.sleep(100);
    }
  }
  throw new Error("PostgreSQL did not become ready within 10 seconds");
}

async function inspectRunning(container: string): Promise<boolean | undefined> {
  try {
    return await podman("inspect", "--format", "{{.State.Running}}", container) === "true";
  } catch (error) {
    if (error instanceof Error && /no such (object|container)/i.test(error.message)) {
      return undefined;
    }
    throw error;
  }
}

async function start(profile: Profile): Promise<State> {
  const existing = await readState();
  if (existing) {
    const running = await inspectRunning(existing.container);
    if (running === true) {
      if (existing.profile === profile) return existing;
    }
    if (running !== undefined) {
      await podman("rm", "--force", existing.container);
    }
    await Bun.file(stateFile).delete();
  }
  const rootless = await podman("info", "--format", "{{.Host.Security.Rootless}}");
  if (rootless !== "true") throw new Error("observatory requires rootless Podman");
  const password = crypto.randomUUID();
  const container = await podman(
    "run", "--detach", "--name", `dsql-observatory-${crypto.randomUUID()}`,
    "--label", "dev.dsql.observatory=true", "--publish", "127.0.0.1::5432",
    "--tmpfs", "/var/lib/postgresql/data:rw,noexec,nosuid,size=2g",
    "--env", `POSTGRES_PASSWORD=${password}`, "--env", "POSTGRES_DB=observatory",
    "--env", "TZ=UTC", image,
  );
  try {
    const mapping = await podman("port", container, "5432/tcp");
    const port = mapping.slice(mapping.lastIndexOf(":") + 1);
    if (!/^\d+$/.test(port)) throw new Error(`invalid Podman port mapping: ${mapping}`);
    const url = `postgres://postgres:${password}@127.0.0.1:${port}/observatory`;
    const sql = await waitReady(url);
    try {
      await sql.file(schemaFile);
      await seed(sql, profile);
    } finally {
      await sql.close();
    }
    const state = { container, url, profile };
    await writeFile(stateFile, `${JSON.stringify(state, null, 2)}\n`, {
      flag: "wx",
      mode: 0o600,
    });
    return state;
  } catch (error) {
    await podman("rm", "--force", container).catch(() => undefined);
    throw error;
  }
}

async function stop(): Promise<void> {
  const state = await readState();
  if (!state) return;
  await podman("rm", "--force", state.container).catch(() => undefined);
  await Bun.file(stateFile).delete();
}

const command = process.argv[2] ?? "status";
const profile = (process.argv.find((value) => value.startsWith("--profile="))?.split("=")[1] ?? "correctness") as Profile;
if (!(["correctness", "small", "medium", "large"] as string[]).includes(profile)) {
  throw new Error(`unknown profile: ${profile}`);
}

if (command === "start") console.log((await start(profile)).url);
else if (command === "reset") { await stop(); console.log((await start(profile)).url); }
else if (command === "stop") await stop();
else if (command === "url") {
  const state = await readState();
  if (!state || await inspectRunning(state.container) !== true) throw new Error("database is stopped");
  console.log(state.url);
}
else if (command === "status") {
  const state = await readState();
  console.log(state && await inspectRunning(state.container) === true ? state : { status: "stopped" });
}
else throw new Error(`unknown command: ${command}`);
