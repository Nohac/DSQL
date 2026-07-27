import { existsSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "bun:test";
import type { DsqlCompileResult } from "../src/daemon";
import {
  loadBuildArtifacts,
  renderDsqlCompileResult,
  type BuildArtifacts,
  type DsqlRendererContext,
} from "../src/node";
import {
  defineDsqlProject,
  targetOutput,
  typescriptDefinitions,
  type DsqlProjectGenerator,
} from "../src/renderer";

const CONTRACT_HASH = {
  algorithm: "sha256" as const,
  value: "1".repeat(64),
};

const project = defineDsqlProject({
  contractHash: CONTRACT_HASH,
  scopes: {
    api: { imports: ["shared"] },
    frontend: { imports: ["shared"] },
    shared: { imports: [] },
  },
  targets: ["api", "frontend"],
  directives: {},
});

test("version 4 manifests are rejected explicitly", () => {
  const root = mkdtempSync(join(tmpdir(), "dsql-old-manifest-"));
  const manifest = join(root, "manifest.json");
  writeFileSync(
    manifest,
    JSON.stringify({
      version: 4,
      generationId: 1,
      operations: [],
      fragments: [],
    }),
  );

  expect(() => loadBuildArtifacts(manifest)).toThrow(
    "unsupported dsql build manifest version 4; expected 5",
  );
});

test("project renderer dispatches ordered generators only to terminal targets", async () => {
  const calls: string[] = [];
  const first = project.generator({
    name: "first",
    render({ target, files }) {
      calls.push(`${target}:first`);
      files.write("first.ts", `export const target = ${JSON.stringify(target)};\n`);
    },
  });
  const second = project.generator({
    name: "second",
    render({ target, files }) {
      calls.push(`${target}:second`);
      files.write("second.ts", "export {};\n");
    },
  });
  const renderer = project.renderer({
    output: targetOutput("src/generated/dsql"),
    targets: {
      api: { generators: [first, second] },
      frontend: project.ignore(),
    },
  });

  const rendered = await renderer.render(
    rendererContext([
      emptyArtifacts("api", true),
      emptyArtifacts("frontend", true),
      emptyArtifacts("shared", false),
    ]),
  );

  expect(calls).toEqual(["api:first", "api:second"]);
  expect(rendered.files).toEqual([
    {
      path: "src/generated/dsql/api/first.ts",
      contents: 'export const target = "api";\n',
    },
    {
      path: "src/generated/dsql/api/second.ts",
      contents: "export {};\n",
    },
  ]);
});

test("project renderer rejects stale targets and file collisions", async () => {
  const renderer = project.renderer({
    output: targetOutput("src/generated/dsql"),
    targets: {
      api: {
        generators: [
          project.generator({
            name: "one",
            render({ files }) {
              files.write("same.ts", "one\n");
            },
          }),
          project.generator({
            name: "two",
            render({ files }) {
              files.write("same.ts", "two\n");
            },
          }),
        ],
      },
      frontend: project.ignore(),
    },
  });
  await expect(
    renderer.render(
      rendererContext([
        emptyArtifacts("api", true),
        emptyArtifacts("frontend", true),
        emptyArtifacts("unexpected", true),
      ]),
    ),
  ).rejects.toThrow("unexpected unexpected");
  await expect(
    renderer.render(
      rendererContext([
        emptyArtifacts("api", true),
        emptyArtifacts("frontend", true),
        emptyArtifacts("shared", false),
      ]),
    ),
  ).rejects.toThrow("dsql generator two failed for target api");
});

test("project renderer validates target decisions before daemon startup", () => {
  const restricted = project.generator({
    name: "frontend-only",
    targets: ["frontend"],
    render() {},
  });
  expect(() =>
    project.renderer({
      output: targetOutput("src/generated/dsql"),
      targets: {
        api: {
          generators: [restricted as unknown as DsqlProjectGenerator<"api">],
        },
        frontend: project.ignore(),
      },
    }),
  ).toThrow(
    "dsql generator frontend-only cannot run for target api",
  );
  expect(() =>
    project.renderer({
      output: targetOutput("src/generated/dsql"),
      targets: {
        api: project.ignore(),
        extra: project.ignore(),
      } as never,
    }),
  ).toThrow("missing frontend; unknown extra");
});

test("compiler contract mismatch fails before rendering or publishing", async () => {
  const root = mkdtempSync(join(tmpdir(), "dsql-project-renderer-"));
  const sentinel = join(root, "sentinel.ts");
  writeFileSync(sentinel, "authored\n");
  let rendered = false;
  const renderer = {
    projectContractHash: CONTRACT_HASH,
    ownedRoots: ["src/generated/dsql"],
    async render() {
      rendered = true;
      return {
        modules: [],
        ownedRoots: ["src/generated/dsql"],
        files: [],
      };
    },
  };
  const result = compileResult({
    algorithm: "sha256",
    value: "2".repeat(64),
  });

  await expect(
    renderDsqlCompileResult(renderer, result, {
      projectBase: root,
      refresh: async () => result,
      environment: () => ({
        mode: "test",
        command: "build",
        outputMode: "readable",
      }),
    }),
  ).rejects.toThrow("run `dsql project sync`");
  expect(rendered).toBe(false);
  expect(existsSync(sentinel)).toBe(true);
});

test("a daemon without project contracts requires an upgrade, not project sync", async () => {
  const result = {
    ...compileResult(CONTRACT_HASH),
    projectContractHash: undefined,
  } as unknown as DsqlCompileResult;
  const renderer = project.renderer({
    output: targetOutput("src/generated/dsql"),
    targets: {
      api: project.ignore(),
      frontend: project.ignore(),
    },
  });

  await expect(
    renderDsqlCompileResult(renderer, result, {
      projectBase: "/tmp/project",
      refresh: async () => result,
      environment: () => ({
        mode: "test",
        command: "build",
        outputMode: "readable",
      }),
    }),
  ).rejects.toThrow("upgrade or restart the dsql daemon");
});

test("embedded callsites require definitions and receive their target mapping", async () => {
  const root = mkdtempSync(join(tmpdir(), "dsql-project-renderer-"));
  const operation = operationMetadata();
  const api = {
    ...emptyArtifacts("api", true),
    manifest: {
      version: 5 as const,
      generationId: 1,
      operations: [
        {
          name: "MovieLookup",
          kind: "query" as const,
          path: "operations/MovieLookup.json",
          hash: "0".repeat(64),
          source: "queries/movie.dsql",
        },
      ],
      fragments: [],
    },
    operations: [operation],
    operationsByName: new Map([[operation.name, operation]]),
    artifactIds: new Map([
      ["operation/MovieLookup", "api/operation/MovieLookup"],
    ]),
  };
  const renderer = project.renderer({
    output: targetOutput("src/generated/dsql"),
    targets: {
      api: {
        generators: [project.generator(typescriptDefinitions())],
      },
      frontend: project.ignore(),
    },
  });
  const context = rendererContext([
    api,
    emptyArtifacts("frontend", true),
    emptyArtifacts("shared", false),
  ], [
    {
      path: "src/Movie.ts",
      resolver: "typescript",
      contentHash: { algorithm: "sha256", value: "0".repeat(64) },
      expressions: [
        {
          range: { start: 0, end: 1 },
          target: "api/operation/MovieLookup",
        },
      ],
    },
  ]);
  const rendered = await renderer.render(context);

  expect(rendered.modules).toEqual([
    {
      id: "api/operation/MovieLookup",
      module: "src/generated/dsql/api/queries/MovieLookup.ts",
      export: "MovieLookupOperation",
    },
  ]);
  expect(
    rendered.files.some(
      (file) => file.path === "src/generated/dsql/api/queries/MovieLookup.ts",
    ),
  ).toBe(true);
});

function rendererContext(
  artifactGroups: readonly BuildArtifacts[],
  callsites: DsqlCompileResult["callsites"] = [],
): DsqlRendererContext {
  return {
    projectBase: "/tmp/project",
    result: { ...compileResult(CONTRACT_HASH), callsites },
    artifacts: {
      ...emptyArtifacts("root", false),
      artifactGroups,
    },
    embeddedSources: new Map(),
    mode: "test",
    command: "build",
    outputMode: "readable",
  };
}

function emptyArtifacts(
  name: string,
  generationTarget: boolean,
): BuildArtifacts {
  const imports = name === "api" || name === "frontend" ? ["shared"] : [];
  return {
    manifestPath: "dsql/build/manifest.1.json",
    currentManifestPath: "dsql/build/manifest.json",
    scopes: [{ name, imports, generationTarget }],
    sourceFileScopes: [],
    artifactGroups: [],
    manifest: {
      version: 5,
      generationId: 1,
      operations: [],
      fragments: [],
    },
    operations: [],
    operationsByName: new Map(),
    fragments: [],
    fragmentsByName: new Map(),
    artifactIds: new Map(),
  };
}

function compileResult(
  projectContractHash: DsqlCompileResult["projectContractHash"],
): DsqlCompileResult {
  return {
    generationId: 1,
    changed: true,
    manifestPath: "dsql/build/manifest.1.json",
    currentManifestPath: "dsql/build/manifest.json",
    projectContractHash,
    manifest: {
      version: 5,
      generationId: 1,
      operations: [],
      fragments: [],
    },
    artifacts: [],
    groups: [],
    sourceFileScopes: [],
    callsites: [],
    diagnostics: [],
  };
}

function operationMetadata(): BuildArtifacts["operations"][number] {
  return {
    name: "MovieLookup",
    kind: "query",
    sql: {
      dialect: "postgres",
      text: "select 1",
      compact_text: "select 1",
      parameters: [],
      variants: [],
    },
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
        id: "MovieLookup",
        file: "queries/movie.dsql",
        range: { start: 0, end: 1 },
      },
    ],
  };
}
