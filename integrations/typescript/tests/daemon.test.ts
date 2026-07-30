import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "bun:test";
import {
  DsqlDaemonClient,
  DsqlDaemonError,
  DsqlDaemonSessionError,
} from "../src/daemon";

const FAKE = join(import.meta.dir, "fixtures/fake-daemon.ts");

type Step = Record<string, unknown>;

function harness(steps: Step[]): {
  client: DsqlDaemonClient;
  scriptPath: string;
  logPath: string;
  invalidations: () => number;
  requests: () => Array<{ id: number; method: string; params?: unknown }>;
} {
  const dir = mkdtempSync(join(tmpdir(), "dsql-daemon-test-"));
  const scriptPath = join(dir, "script.json");
  const logPath = join(dir, "log.jsonl");
  writeFileSync(scriptPath, JSON.stringify(steps));
  writeFileSync(logPath, "");
  let invalidated = 0;
  const client = new DsqlDaemonClient({
    root: dir,
    excludeRoots: ["src/generated/dsql"],
    daemon: {
      command: "bun",
      // argv configuration keeps parallel test files isolated.
      args: [FAKE, scriptPath, logPath],
      cwd: dir,
    },
    onInvalidate: () => {
      invalidated += 1;
    },
  });
  return {
    client,
    scriptPath,
    logPath,
    invalidations: () => invalidated,
    requests: () =>
      readFileSync(logPath, "utf8")
        .split("\n")
        .filter(Boolean)
        .map((line) => JSON.parse(line)),
  };
}

const INITIALIZE = {
  expectMethod: "initialize",
  response: {
    result: {
      protocolVersion: 1,
      projectBase: "/tmp/project",
      configPath: "dsql/dsql.toml",
      schemaDir: "dsql/schema",
      overlaysDir: "dsql/overlays",
      buildDir: "dsql/build",
      generatorOutputs: ["src/generated"],
      diagnosticLevel: "info",
    },
  },
};

const EMPTY_RESULT = {
  generationId: 1,
  changed: true,
  manifestPath: "dsql/build/manifest.1.json",
  currentManifestPath: "dsql/build/manifest.json",
  manifest: { version: 7, generationId: 1, operations: [], fragments: [] },
  artifacts: [],
  groups: [],
  sourceFileScopes: [],
  callsites: [],
  diagnostics: [],
};

test("initializes lazily, compiles, and replays carry changed:false", async () => {
  const h = harness([
    INITIALIZE,
    { expectMethod: "compile", response: { result: EMPTY_RESULT } },
    {
      expectMethod: "filesChanged",
      response: { result: { ...EMPTY_RESULT, changed: false } },
    },
  ]);

  const compiled = await h.client.compile();
  expect(compiled.generationId).toBe(1);
  expect(compiled.changed).toBe(true);
  expect(h.client.info?.projectBase).toBe("/tmp/project");
  expect(h.client.info?.generatorOutputs).toEqual(["src/generated"]);

  const replay = await h.client.filesChanged(["README.md"]);
  expect(replay.changed).toBe(false);

  const requests = h.requests();
  expect(requests.map((request) => request.method)).toEqual([
    "initialize",
    "compile",
    "filesChanged",
  ]);
  const init = requests[0]?.params as Record<string, unknown>;
  expect(init.protocolVersion).toBe(1);
  expect(init.excludeRoots).toEqual(["src/generated/dsql"]);
  expect(init.diagnosticLevel).toBe("info");
  expect((requests[2]?.params as Record<string, unknown>).paths).toEqual(["README.md"]);
  await h.client.shutdown();
}, 30_000);

test("error responses preserve code and structured data", async () => {
  const h = harness([
    INITIALIZE,
    {
      expectMethod: "compile",
      response: {
        error: {
          code: "Diagnostics",
          message: "cannot generate while diagnostics contain errors",
          data: {
            diagnostics: [
              {
                file: "queries/broken.dsql",
                range: { start: 4, end: 9 },
                severity: "Error",
                source: "Check",
                code: "UnknownTable",
                message: "unknown table",
              },
            ],
          },
        },
      },
    },
  ]);

  const error = await h.client.compile().catch((caught: unknown) => caught);
  expect(error).toBeInstanceOf(DsqlDaemonError);
  const daemonError = error as DsqlDaemonError;
  expect(daemonError.code).toBe("Diagnostics");
  expect(daemonError.diagnostics).toHaveLength(1);
  expect(daemonError.diagnostics[0]?.code).toBe("UnknownTable");
  await h.client.shutdown();
}, 30_000);

test("unexpected death rejects in-flight work, invalidates, and restarts warm", async () => {
  const h = harness([INITIALIZE, { exit: 1 }]);

  const error = await h.client.compile().catch((caught: unknown) => caught);
  expect(error).toBeInstanceOf(DsqlDaemonSessionError);
  expect(h.invalidations()).toBe(1);

  // A fresh spawn re-reads the script; feed it a healthy run.
  writeFileSync(
    h.scriptPath,
    JSON.stringify([
      INITIALIZE,
      { expectMethod: "compile", response: { result: EMPTY_RESULT } },
    ]),
  );
  const compiled = await h.client.compile();
  expect(compiled.generationId).toBe(1);
  await h.client.shutdown();
}, 30_000);

test("malformed daemon output is a fatal session failure", async () => {
  const h = harness([INITIALIZE, { garbage: true }]);
  const error = await h.client.compile().catch((caught: unknown) => caught);
  expect(error).toBeInstanceOf(DsqlDaemonSessionError);
  expect((error as Error).message).toContain("malformed JSON");
  expect(h.invalidations()).toBe(1);
}, 30_000);

test("out-of-order response ids are a fatal session failure", async () => {
  const h = harness([INITIALIZE, { wrongId: true, response: { result: EMPTY_RESULT } }]);
  const error = await h.client.compile().catch((caught: unknown) => caught);
  expect(error).toBeInstanceOf(DsqlDaemonSessionError);
  expect((error as Error).message).toContain("out of order");
}, 30_000);

test("UnsupportedProtocolVersion is fatal without restart", async () => {
  const h = harness([
    {
      expectMethod: "initialize",
      response: {
        error: {
          code: "UnsupportedProtocolVersion",
          message: "daemon speaks protocol 2",
          data: null,
        },
      },
    },
  ]);

  const first = await h.client.compile().catch((caught: unknown) => caught);
  expect(first).toBeInstanceOf(DsqlDaemonError);
  expect((first as DsqlDaemonError).code).toBe("UnsupportedProtocolVersion");

  const second = await h.client.compile().catch((caught: unknown) => caught);
  expect(second).toBe(first);
  // Exactly one spawn: the fatal error short-circuits before respawning.
  expect(h.requests().filter((request) => request.method === "initialize")).toHaveLength(1);
}, 30_000);

test("Internal error responses poison the session", async () => {
  const h = harness([
    INITIALIZE,
    {
      expectMethod: "compile",
      response: { error: { code: "Internal", message: "invariant broke", data: null } },
    },
  ]);
  const error = await h.client.compile().catch((caught: unknown) => caught);
  expect(error).toBeInstanceOf(DsqlDaemonError);
  expect((error as DsqlDaemonError).code).toBe("Internal");
  expect(h.invalidations()).toBe(1);
}, 30_000);

test("shutdown drains queued work and rejects later requests", async () => {
  const h = harness([
    INITIALIZE,
    { expectMethod: "compile", response: { result: EMPTY_RESULT } },
    { expectMethod: "filesChanged", response: { result: EMPTY_RESULT } },
  ]);
  // Queue two requests and shut down immediately: both must complete
  // against the session shutdown then closes.
  const first = h.client.compile();
  const second = h.client.filesChanged(["a.dsql"]);
  const closing = h.client.shutdown();
  const [firstResult, secondResult] = await Promise.all([first, second]);
  await closing;
  expect(firstResult.changed).toBe(true);
  expect(secondResult.changed).toBe(true);
  expect(h.requests().map((request) => request.method)).toEqual([
    "initialize",
    "compile",
    "filesChanged",
    "shutdown",
  ]);

  // Work arriving after shutdown is rejected, never queued.
  const late = await h.client.compile().catch((caught: unknown) => caught);
  expect(late).toBeInstanceOf(DsqlDaemonSessionError);
  expect((late as Error).message).toContain("shut down");
}, 30_000);

test("shutdown before any spawn stays a no-op but still closes the client", async () => {
  const h = harness([]);
  await h.client.shutdown();
  expect(h.requests()).toHaveLength(0);
  const late = await h.client.compile().catch((caught: unknown) => caught);
  expect(late).toBeInstanceOf(DsqlDaemonSessionError);
}, 30_000);

test("concurrent callers serialize FIFO and shutdown answers last", async () => {
  const h = harness([
    INITIALIZE,
    { expectMethod: "compile", response: { result: EMPTY_RESULT } },
    { expectMethod: "filesChanged", response: { result: EMPTY_RESULT } },
  ]);
  const [first, second] = await Promise.all([
    h.client.compile(),
    h.client.filesChanged(["a.dsql"]),
  ]);
  expect(first.changed).toBe(true);
  expect(second.changed).toBe(true);
  await h.client.shutdown();
  const methods = h.requests().map((request) => request.method);
  expect(methods).toEqual(["initialize", "compile", "filesChanged", "shutdown"]);
}, 30_000);
