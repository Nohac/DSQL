import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { expect, test } from "bun:test";
import { DsqlDaemonError } from "../src/daemon";
import { sha256Hex, type DsqlRenderer, type DsqlRendererContext } from "../src/node";
import { DsqlRewriteError } from "../src/rewrite";
import { dsql as dsqlPlugin } from "../src/vite";

const FAKE = join(import.meta.dir, "fixtures/fake-daemon.ts");
const HOST_PATH = "src/components/Panel.tsx";

const HOST = `"use client";

export const TitlePanelQuery = dsql(\`query TitlePanel {
  title {
    id
  }
}\`);
`;

function byteIndexOf(code: string, needle: string, from = 0): number {
  const index = code.indexOf(needle, from);
  if (index < 0) {
    throw new Error(`fixture is missing ${needle}`);
  }
  return Buffer.byteLength(code.slice(0, index), "utf8");
}

/** A compile result whose callsite/content ranges match `hostCode`. */
function resultFor(
  hostCode: string,
  options: {
    generationId?: number;
    changed?: boolean;
    withCallsite?: boolean;
    path?: string;
    resolver?: string;
  } = {},
): Record<string, unknown> {
  const withCallsite = options.withCallsite ?? true;
  const path = options.path ?? HOST_PATH;
  const expressionStart = withCallsite ? byteIndexOf(hostCode, "dsql(`") : 0;
  const expressionEnd = withCallsite
    ? byteIndexOf(hostCode, "`)", expressionStart) + Buffer.byteLength("`)")
    : 0;
  const contentStart = expressionStart + Buffer.byteLength("dsql(`");
  const contentEnd = expressionEnd - Buffer.byteLength("`)");
  const operation = {
    name: "TitlePanel",
    kind: "query",
    sql: { dialect: "postgres", text: "select 1", parameters: [], variants: [] },
    result: { fields: [] },
    params: [],
    input: [],
    context: [],
    dynamic_inputs: [],
    policies: [],
    handoffs: [],
    fragment_spreads: [],
    source_map: [
      {
        id: "TitlePanel",
        file: path,
        range: { start: expressionStart, end: expressionEnd },
        ...(withCallsite
          ? { content_range: { start: contentStart, end: contentEnd } }
          : {}),
      },
    ],
  };
  return {
    generationId: options.generationId ?? 1,
    changed: options.changed ?? true,
    manifestPath: "dsql/build/manifest.1.json",
    currentManifestPath: "dsql/build/manifest.json",
    manifest: {
      version: 2,
      generationId: options.generationId ?? 1,
      operations: [
        {
          name: "TitlePanel",
          kind: "query",
          path: "operations/TitlePanel.0000000000000000.json",
          hash: "0".repeat(64),
          source: path,
        },
      ],
      fragments: [],
    },
    artifacts: withCallsite
      ? [
          {
            id: "frontend/operation/TitlePanel",
            kind: "operation",
            scope: "frontend",
            metadata: operation,
          },
        ]
      : [],
    groups: withCallsite
      ? [
          {
            name: "frontend",
            imports: [],
            artifacts: ["frontend/operation/TitlePanel"],
          },
        ]
      : [],
    sourceFileScopes: withCallsite ? [{ path, scope: "frontend" }] : [],
    callsites: withCallsite
      ? [
          {
            path,
            resolver: options.resolver ?? "typescript",
            contentHash: {
              algorithm: "sha256",
              value: sha256Hex(Buffer.from(hostCode, "utf8")),
            },
            expressions: [
              {
                range: { start: expressionStart, end: expressionEnd },
                target: "frontend/operation/TitlePanel",
              },
            ],
          },
        ]
      : [],
    diagnostics: [],
  };
}

type Harness = {
  base: string;
  plugin: ReturnType<typeof dsqlPlugin>;
  hostAbsolute: string;
  renders: DsqlRendererContext[];
  logs: { error: string[]; info: string[]; warn: string[] };
  scriptPath: string;
  setRendererFailure: (message: string | null) => void;
  requests: () => Array<{ method: string; params?: { paths?: string[] } }>;
};

function harness(
  makeSteps: (base: string) => unknown[],
  hostCode: string = HOST,
  hostPath: string = HOST_PATH,
): Harness {
  const base = mkdtempSync(join(tmpdir(), "dsql-vite-test-"));
  const hostAbsolute = join(base, hostPath);
  mkdirSync(dirname(hostAbsolute), { recursive: true });
  writeFileSync(hostAbsolute, hostCode);

  const scriptPath = join(base, "script.json");
  const logPath = join(base, "log.jsonl");
  writeFileSync(scriptPath, JSON.stringify(makeSteps(base)));
  writeFileSync(logPath, "");

  const renders: DsqlRendererContext[] = [];
  const logs = { error: [] as string[], info: [] as string[], warn: [] as string[] };
  let failure: string | null = null;
  const renderer: DsqlRenderer = {
    ownedRoots: ["src/generated/dsql"],
    async render(context) {
      renders.push(context);
      if (failure) {
        throw new Error(failure);
      }
      return {
        modules: [
          {
            id: "frontend/operation/TitlePanel",
            module: "src/generated/dsql/queries/TitlePanel.ts",
            export: "TitlePanelOperation",
          },
        ],
        ownedRoots: ["src/generated/dsql"],
        files: ["src/generated/dsql/queries/TitlePanel.ts"],
      };
    },
  };
  const plugin = dsqlPlugin({
    renderer,
    root: base,
    daemon: { command: "bun", args: [FAKE, scriptPath, logPath], cwd: base },
    fullReload: false,
  });
  const fakeConfig = {
    root: base,
    mode: "development",
    command: "serve" as const,
    logger: {
      error(message: string) {
        logs.error.push(message);
      },
      info(message: string) {
        logs.info.push(message);
      },
      warn(message: string) {
        logs.warn.push(message);
      },
    },
  };
  (plugin.configResolved as (config: unknown) => void)(fakeConfig);
  return {
    base,
    plugin,
    hostAbsolute,
    logs,
    renders,
    scriptPath,
    setRendererFailure: (message) => {
      failure = message;
    },
    requests: () =>
      readFileSync(logPath, "utf8")
        .split("\n")
        .filter(Boolean)
        .map((line) => JSON.parse(line)),
  };
}

/** A minimal dev server capturing the plugin's watcher listener. */
function fakeServer(): {
  server: unknown;
  emitAll: (file: string) => void;
} {
  const handlers: Array<(event: string, file: string) => void> = [];
  const server = {
    watcher: {
      add() {},
      on(event: string, handler: (event: string, file: string) => void) {
        if (event === "all") {
          handlers.push(handler);
        }
      },
    },
    ws: { send() {}, on() {} },
  };
  return {
    server,
    emitAll: (file) => {
      for (const handler of handlers) {
        handler("change", file);
      }
    },
  };
}

function initializeStep(base: string): unknown {
  return {
    expectMethod: "initialize",
    response: {
      result: {
        protocolVersion: 1,
        projectBase: base,
        configPath: "dsql/dsql.toml",
        schemaDir: "dsql/schema",
        buildDir: "dsql/build",
        generatorOutputs: [],
      },
    },
  };
}

const transform = (h: Harness, code: string, id?: string) =>
  (h.plugin.transform as (code: string, id: string) => Promise<{ code: string } | null>)(
    code,
    id ?? h.hostAbsolute,
  );

const hotUpdate = (h: Harness, file: string) =>
  (h.plugin.handleHotUpdate as (context: unknown) => Promise<unknown[] | undefined>)({
    file,
  });

test("transforms callsites from daemon ranges after compile and render", async () => {
  const h = harness((base) => [
    initializeStep(base),
    { expectMethod: "compile", response: { result: resultFor(HOST) } },
  ]);

  const output = await transform(h, HOST);
  expect(output).not.toBeNull();
  expect(output?.code).toContain(
    'import { TitlePanelOperation as __dsql_TitlePanelOperation } from "/src/generated/dsql/queries/TitlePanel.ts";',
  );
  expect(output?.code).toContain("export const TitlePanelQuery = __dsql_TitlePanelOperation;");
  expect(output?.code).not.toContain("dsql(`");
  // The import respects the "use client" prologue.
  expect(output?.code.startsWith('"use client";\n\nimport { TitlePanelOperation')).toBe(true);

  // The renderer got the extractor-sliced embedded source, hash-verified.
  expect(h.renders).toHaveLength(1);
  expect(h.renders[0]?.embeddedSources.get("operation/TitlePanel")).toBe(
    "query TitlePanel {\n  title {\n    id\n  }\n}",
  );
  expect(h.renders[0]?.command).toBe("serve");
}, 30_000);

test("transforms configured TypeScript callsites on nonstandard extensions", async () => {
  const hostPath = "src/components/Panel.component";
  const h = harness(
    (base) => [
      initializeStep(base),
      {
        expectMethod: "compile",
        response: { result: resultFor(HOST, { path: hostPath }) },
      },
    ],
    HOST,
    hostPath,
  );

  const output = await transform(h, HOST);
  expect(output?.code).toContain("__dsql_TitlePanelOperation");
}, 30_000);

test("rejects callsites owned by another resolver", async () => {
  const h = harness((base) => [
    initializeStep(base),
    {
      expectMethod: "compile",
      response: { result: resultFor(HOST, { resolver: "custom" }) },
    },
  ]);

  const error = await transform(h, HOST).catch((caught: unknown) => caught);
  expect(error).toBeInstanceOf(DsqlRewriteError);
  expect((error as Error).message).toContain('resolver "custom" is not supported');
}, 30_000);

test("successful diagnostics stay quiet while generation errors surface", async () => {
  const warning = {
    file: HOST_PATH,
    range: { start: 0, end: 1 },
    severity: "Warning",
    source: "Lint",
    code: "UnindexedScanColumn",
    message: "scan can be slow",
  };
  const quiet = harness((base) => [
    initializeStep(base),
    {
      expectMethod: "compile",
      response: {
        result: { ...resultFor(HOST), diagnostics: [warning] },
      },
    },
  ]);

  expect(await transform(quiet, HOST)).not.toBeNull();
  expect(quiet.logs).toEqual({ error: [], info: [], warn: [] });

  const broken = harness((base) => [
    initializeStep(base),
    {
      expectMethod: "compile",
      response: {
        error: {
          code: "Diagnostics",
          message: "cannot generate while diagnostics contain errors",
          data: { diagnostics: [{ ...warning, severity: "Error", source: "Check" }] },
        },
      },
    },
  ]);
  expect(await transform(broken, HOST)).toBeNull();
  expect(broken.logs.warn).toEqual([]);
  expect(broken.logs.info).toEqual([]);
  expect(broken.logs.error.join("\n")).toContain("cannot generate");
}, 30_000);

test("files without callsites and non-project ids pass through untouched", async () => {
  const h = harness((base) => [
    initializeStep(base),
    { expectMethod: "compile", response: { result: resultFor(HOST) } },
  ]);
  expect(await transform(h, "const x = 1;", join(h.base, "src/other.ts"))).toBeNull();
  expect(await transform(h, "const x = 1;", "\0virtual:module")).toBeNull();
  expect(await transform(h, "body { color: red }", join(h.base, "src/app.css"))).toBeNull();
}, 30_000);

test("a stale buffer refreshes once, then transforms against the fresh result", async () => {
  const edited = HOST.replace("id\n", "id\n    title\n");
  const h = harness((base) => [
    initializeStep(base),
    { expectMethod: "compile", response: { result: resultFor(HOST) } },
    {
      expectMethod: "filesChanged",
      response: { result: resultFor(edited, { generationId: 2 }) },
    },
  ]);
  // Initial compile+render against the on-disk HOST.
  expect((await transform(h, HOST))?.code).toContain("__dsql_TitlePanelOperation");

  // The file is saved with an edit the daemon has not seen yet; the
  // transform's freshness check triggers the refresh.
  writeFileSync(h.hostAbsolute, edited);
  const output = await transform(h, edited);
  expect(output?.code).toContain("__dsql_TitlePanelOperation;");
  expect(h.renders).toHaveLength(2);
}, 30_000);

test("a persistently mismatched buffer fails deterministically", async () => {
  const h = harness((base) => [
    initializeStep(base),
    { expectMethod: "compile", response: { result: resultFor(HOST) } },
    // The refresh replays the same generation: nothing changed on disk.
    {
      expectMethod: "filesChanged",
      response: { result: resultFor(HOST, { changed: false }) },
    },
  ]);

  const unsaved = HOST.replace("TitlePanelQuery", "UnsavedRename");
  const error = await transform(h, unsaved).catch((caught: unknown) => caught);
  expect(error).toBeInstanceOf(DsqlRewriteError);
  expect((error as Error).message).toContain("does not match the saved file");
}, 30_000);

test("when the callsites leave the file, the refresh declines to transform", async () => {
  const withoutCallsite = 'export const nothing = 1;\n';
  const h = harness((base) => [
    initializeStep(base),
    { expectMethod: "compile", response: { result: resultFor(HOST) } },
    {
      expectMethod: "filesChanged",
      response: {
        result: resultFor(withoutCallsite, { generationId: 2, withCallsite: false }),
      },
    },
  ]);
  writeFileSync(h.hostAbsolute, withoutCallsite);

  expect(await transform(h, withoutCallsite)).toBeNull();
}, 30_000);

test("a failed freshness refresh fails the transform loudly", async () => {
  const h = harness((base) => [
    initializeStep(base),
    { expectMethod: "compile", response: { result: resultFor(HOST) } },
    {
      expectMethod: "filesChanged",
      response: {
        error: { code: "Diagnostics", message: "edit broke a query", data: { diagnostics: [] } },
      },
    },
  ]);
  expect((await transform(h, HOST))?.code).toContain("__dsql_TitlePanelOperation");

  // The edited buffer needs a refresh, and the refresh fails: the raw
  // dsql() expression must NOT pass through as if the callsite left.
  const edited = HOST.replace("TitlePanelQuery", "EditedQuery");
  const error = await transform(h, edited).catch((caught: unknown) => caught);
  expect(error).toBeInstanceOf(DsqlDaemonError);
  expect((error as DsqlDaemonError).code).toBe("Diagnostics");
}, 30_000);

test("a failed config reload clears cached state until a compile succeeds", async () => {
  const h = harness((base) => [
    initializeStep(base),
    { expectMethod: "compile", response: { result: resultFor(HOST) } },
  ]);
  expect((await transform(h, HOST))?.code).toContain("__dsql_TitlePanelOperation");

  // The respawned daemon (after the config-change restart) reads the
  // script fresh: initialize succeeds, but the project no longer loads.
  const failing = {
    expectMethod: "compile",
    response: {
      error: { code: "ProjectLoadFailed", message: "broken dsql.toml", data: null },
    },
  };
  writeFileSync(
    h.scriptPath,
    JSON.stringify([initializeStep(h.base), failing, failing]),
  );
  const outcome = await hotUpdate(h, join(h.base, "dsql/dsql.toml"));
  expect(outcome).toEqual([]);

  // Old-scope transforms must not stay active behind a failed reload.
  expect(await transform(h, HOST)).toBeNull();
  expect(h.renders).toHaveLength(1);
}, 30_000);

test("nothing transforms before the first successful compile", async () => {
  const h = harness((base) => [
    initializeStep(base),
    {
      expectMethod: "compile",
      response: {
        error: { code: "Diagnostics", message: "errors", data: { diagnostics: [] } },
      },
    },
  ]);
  expect(await transform(h, HOST)).toBeNull();
  expect(h.renders).toHaveLength(0);
}, 30_000);

test("a missing render mapping names the artifact id", async () => {
  const h = harness((base) => [
    initializeStep(base),
    { expectMethod: "compile", response: { result: resultFor(HOST) } },
  ]);
  // A renderer that maps nothing: valid map, useless for this callsite.
  const empty = dsqlPlugin({
    renderer: {
      ownedRoots: ["src/generated/dsql"],
      async render() {
        return { modules: [], ownedRoots: ["src/generated/dsql"], files: [] };
      },
    },
    root: h.base,
    daemon: { command: "bun", args: [FAKE, h.scriptPath], cwd: h.base },
    fullReload: false,
  });
  (empty.configResolved as (config: unknown) => void)({
    root: h.base,
    mode: "development",
    command: "serve",
    logger: { info() {}, warn() {}, error() {} },
  });
  const error = await (
    empty.transform as (code: string, id: string) => Promise<unknown>
  )(HOST, h.hostAbsolute).catch((caught: unknown) => caught);
  expect(error).toBeInstanceOf(DsqlRewriteError);
  expect((error as Error).message).toContain(
    "no render mapping for artifact frontend/operation/TitlePanel",
  );
}, 30_000);

test("renderer failure keeps the previous state transformable", async () => {
  const h = harness((base) => [
    initializeStep(base),
    { expectMethod: "compile", response: { result: resultFor(HOST) } },
    {
      expectMethod: "filesChanged",
      response: { result: resultFor(HOST, { generationId: 2 }) },
    },
  ]);

  expect((await transform(h, HOST))?.code).toContain("__dsql_TitlePanelOperation");

  h.setRendererFailure("renderer exploded");
  const outcome = await hotUpdate(h, h.hostAbsolute);
  expect(outcome).toEqual([]);

  // The old state survived the failed swap: the same buffer still
  // transforms against generation 1.
  h.setRendererFailure(null);
  expect((await transform(h, HOST))?.code).toContain("__dsql_TitlePanelOperation");
  expect(h.renders).toHaveLength(2);
}, 30_000);

test("irrelevant files keep normal HMR on changed:false replays", async () => {
  const h = harness((base) => [
    initializeStep(base),
    { expectMethod: "compile", response: { result: resultFor(HOST) } },
    {
      expectMethod: "filesChanged",
      response: { result: resultFor(HOST, { changed: false }) },
    },
  ]);
  await transform(h, HOST);

  const readme = join(h.base, "README.md");
  writeFileSync(readme, "# hi\n");
  expect(await hotUpdate(h, readme)).toBeUndefined();
}, 30_000);

test("one fs event delivers once regardless of listener order", async () => {
  const unchangedReplay = {
    expectMethod: "filesChanged",
    response: { result: resultFor(HOST, { changed: false }) },
  };
  const h = harness((base) => [
    initializeStep(base),
    { expectMethod: "compile", response: { result: resultFor(HOST) } },
    unchangedReplay,
    unchangedReplay,
  ]);
  await transform(h, HOST);
  const { server, emitAll } = fakeServer();
  (h.plugin.configureServer as (server: unknown) => void)(server);

  // One fs event fans out to handleHotUpdate AND the watcher listener,
  // in either order — the daemon must see exactly one delivery each.
  const hmrFirst = hotUpdate(h, h.hostAbsolute);
  emitAll(h.hostAbsolute);
  await hmrFirst;
  expect(
    h.requests().filter((request) => request.method === "filesChanged"),
  ).toHaveLength(1);

  emitAll(h.hostAbsolute);
  const watcherFirst = hotUpdate(h, h.hostAbsolute);
  await watcherFirst;
  expect(
    h.requests().filter((request) => request.method === "filesChanged"),
  ).toHaveLength(2);
}, 30_000);

test("renderer-owned paths are swallowed without a daemon round-trip", async () => {
  const h = harness((base) => [
    initializeStep(base),
    { expectMethod: "compile", response: { result: resultFor(HOST) } },
  ]);
  await transform(h, HOST);
  const generated = join(h.base, "src/generated/dsql/queries/TitlePanel.ts");
  expect(await hotUpdate(h, generated)).toEqual([]);
  // Only initialize + compile ever reached the daemon.
  expect(h.renders).toHaveLength(1);
}, 30_000);
