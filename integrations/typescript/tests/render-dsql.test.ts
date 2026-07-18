import {
  existsSync,
  mkdtempSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { expect, test } from "bun:test";
import {
  renderDsql,
  renderMapFromResults,
  resolveEmbeddedSources,
  sha256Hex,
  type BuildArtifacts,
} from "../src/node";

test("renders inline per-definition dsql modules", async () => {
  const root = createRoot();
  const artifacts = createArtifacts(root);

  const result = await renderDsql(artifacts, {
    root,
    queriesDir: "src/generated/dsql/queries",
    embeddedSources: new Map([
      [
        "operation/MovieInfoLookup",
        "query MovieInfoLookup { movie_info(where .id == $id) { id } }",
      ],
    ]),
  });

  expect(result.modules.queries).toBe("./src/generated/dsql/queries/index");
  expect(result.definitions["operation/MovieInfoLookup"]).toEqual({
    name: "MovieInfoLookup",
    kind: "query",
    id: "default/operation/MovieInfoLookup",
    exportName: "MovieInfoLookupOperation",
    operationModule: "./src/generated/dsql/queries/MovieInfoLookup",
    modulePath: "src/generated/dsql/queries/MovieInfoLookup.ts",
    executionModule: "./src/generated/dsql/queries/MovieInfoLookup",
  });

  const operation = readFileSync(
    join(root, "src/generated/dsql/queries/MovieInfoLookup.ts"),
    "utf8",
  );
  expect(operation).toContain("export type MovieInfoLookupResult");
  expect(operation).toContain(
    "export type MovieInfoLookupContext = {\n  user_id: string;\n};",
  );
  expect(operation).toContain("export const MovieInfoLookupOperation");
  expect(operation).toContain("export const MovieInfoLookupExecutionPayload");
  expect(operation).toContain("declare module \"@dsql/typescript/runtime\"");
  expect(operation).toContain(
    "query MovieInfoLookup { movie_info(where .id == $id) { id } }",
  );

  const fragment = readFileSync(
    join(root, "src/generated/dsql/queries/MovieFields.fragment.ts"),
    "utf8",
  );
  expect(fragment).toContain("export const MovieFieldsFragment");

  const renderMap = renderMapFromResults([result], {
    projectBase: root,
    ownedRoots: ["src/generated/dsql"],
  });
  expect(renderMap.modules.map((module) => module.id)).toEqual([
    "default/fragment/MovieFields",
    "default/operation/MovieInfoLookup",
  ]);

  const index = readFileSync(
    join(root, "src/generated/dsql/queries/index.ts"),
    "utf8",
  );
  expect(index).toBe(
    'export { dsql } from "@dsql/typescript/runtime";\nexport type { DsqlDefinition, DsqlExecutionPayload, DsqlFragment, DsqlFragmentDefinition, DsqlFragmentInput, DsqlFragmentParams, DsqlFragmentVariables, DsqlMaterializedQuery, DsqlOperation, DsqlOperationContext, DsqlOperationInput, DsqlOperationParams, DsqlOperationResult, DsqlVariables } from "@dsql/typescript/runtime";\nexport * from "./MovieFields.fragment";\nexport * from "./MovieInfoLookup";\n',
  );
});

test("renders exact numeric and finite/non-finite float wire types", async () => {
  const root = createRoot();
  const operation = {
    ...operationMetadata("NumericMetrics"),
    result: {
      fields: [
        {
          path: "metrics",
          name: "metrics",
          parent_path: "",
          kind: "array",
          data_type: "object",
          nullable: false,
        },
        {
          path: "metrics.amount",
          name: "amount",
          parent_path: "metrics",
          kind: "scalar",
          data_type: "numeric",
          nullable: false,
        },
        {
          path: "metrics.ratio",
          name: "ratio",
          parent_path: "metrics",
          kind: "scalar",
          data_type: "float",
          nullable: true,
        },
      ],
    },
    params: [
      {
        path: "params.minimum",
        data_type: "numeric",
        enum_values: [],
        required: true,
        nullable: false,
      },
      {
        path: "params.threshold",
        data_type: "float",
        enum_values: [],
        required: true,
        nullable: false,
      },
      {
        path: "params.amounts",
        data_type: "numeric",
        collection: true,
        enum_values: [],
        required: true,
        nullable: false,
      },
    ],
  };
  const artifacts = {
    ...createArtifacts(root, { operationNames: [operation.name] }),
    operations: [operation],
    operationsByName: new Map([[operation.name, operation]]),
  } as BuildArtifacts;

  await renderDsql(artifacts, {
    root,
    queriesDir: "src/generated/dsql/queries",
  });
  const rendered = readFileSync(
    join(root, "src/generated/dsql/queries/NumericMetrics.ts"),
    "utf8",
  );

  expect(rendered).toContain("amount: string;");
  expect(rendered).toContain(
    'ratio: number | "NaN" | "Infinity" | "-Infinity" | null;',
  );
  expect(rendered).toContain("minimum: string;");
  expect(rendered).toContain(
    'threshold: number | "NaN" | "Infinity" | "-Infinity";',
  );
  expect(rendered).toContain("amounts: Array<string | null>;");
});

test("renders singular selections as nullable objects while runtime limits stay arrays", async () => {
  const root = createRoot();
  const operation = {
    ...operationMetadata("SelectionShapes"),
    result: {
      fields: [
        {
          path: "singular",
          name: "singular",
          parent_path: "",
          kind: "object",
          data_type: "object",
          nullable: true,
        },
        {
          path: "singular.id",
          name: "id",
          parent_path: "singular",
          kind: "scalar",
          data_type: "int",
          nullable: false,
        },
        {
          path: "runtime",
          name: "runtime",
          parent_path: "",
          kind: "array",
          data_type: "object",
          nullable: false,
        },
        {
          path: "runtime.id",
          name: "id",
          parent_path: "runtime",
          kind: "scalar",
          data_type: "int",
          nullable: false,
        },
      ],
    },
  };
  const artifacts = {
    ...createArtifacts(root, { operationNames: [operation.name] }),
    operations: [operation],
    operationsByName: new Map([[operation.name, operation]]),
  } as BuildArtifacts;

  await renderDsql(artifacts, {
    root,
    queriesDir: "src/generated/dsql/queries",
  });
  const rendered = readFileSync(
    join(root, "src/generated/dsql/queries/SelectionShapes.ts"),
    "utf8",
  );

  expect(rendered).toMatch(/singular:\s+\{\s+id: number;\s+\} \| null;/);
  expect(rendered).toMatch(/runtime: Array<\{\s+id: number;\s+\}>;/);
});

test("renders split query and execution modules with matching filenames", async () => {
  const root = createRoot();
  const artifacts = createArtifacts(root);

  const result = await renderDsql(artifacts, {
    root,
    queriesDir: "src/generated/dsql/queries",
    executionDir: "src/generated/dsql/queries.server",
  });

  expect(result.definitions["operation/MovieInfoLookup"]?.executionModule).toBe(
    "./src/generated/dsql/queries.server/MovieInfoLookup",
  );

  const operation = readFileSync(
    join(root, "src/generated/dsql/queries/MovieInfoLookup.ts"),
    "utf8",
  );
  expect(operation).toContain("export const MovieInfoLookupOperation");
  expect(operation).not.toContain("select * from movie_info");

  const execution = readFileSync(
    join(root, "src/generated/dsql/queries.server/MovieInfoLookup.ts"),
    "utf8",
  );
  expect(execution).toContain(
    'import { MovieInfoLookupOperation } from "../queries/MovieInfoLookup";',
  );
  expect(execution).toContain("select * from movie_info where id = $1");
});

test("registry keys skip content whose cooked value differs from raw bytes", async () => {
  const root = createRoot();
  const artifacts = createArtifacts(root);

  for (const hostile of [
    "query MovieInfoLookup { movie_info(where .id == $id) { id \\n } }",
    "query MovieInfoLookup { movie_info(where .id == ${id}) { id } }",
    "query MovieInfoLookup {\r\n  movie_info { id }\r\n}",
  ]) {
    await renderDsql(artifacts, {
      root,
      queriesDir: "src/generated/dsql/queries",
      embeddedSources: new Map([["operation/MovieInfoLookup", hostile]]),
    });
    const operation = readFileSync(
      join(root, "src/generated/dsql/queries/MovieInfoLookup.ts"),
      "utf8",
    );
    // TypeScript's template-literal types use the cooked string, so raw
    // bytes containing backslashes, ${, or CR would key a type nothing
    // can ever match — no key beats a wrong key.
    expect(operation).not.toContain("declare module");
  }
});

test("query and fragment expressions get their selected registry keys", async () => {
  const root = createRoot();
  const artifacts = createArtifacts(root);

  await renderDsql(artifacts, {
    root,
    queriesDir: "src/generated/dsql/queries",
    embeddedSources: new Map([
      ["operation/MovieInfoLookup", "query MovieInfoLookup { movie_info { id } }"],
      ["fragment/MovieFields", "fragment MovieFields on movie_info { id }"],
    ]),
  });
  const operation = readFileSync(
    join(root, "src/generated/dsql/queries/MovieInfoLookup.ts"),
    "utf8",
  );
  const fragment = readFileSync(
    join(root, "src/generated/dsql/queries/MovieFields.fragment.ts"),
    "utf8",
  );
  expect(operation).toContain("declare module");
  expect(fragment).toContain("declare module");
  expect(fragment).toContain("fragment MovieFields on movie_info { id }");
  expect(fragment).toContain("typeof MovieFieldsFragment");
});

test("same-named queries and fragments keep distinct render mappings", async () => {
  const root = createRoot();
  const result = await renderDsql(createArtifacts(root, { operationNames: ["MovieFields"] }), {
    root,
    queriesDir: "src/generated/dsql/queries",
  });

  expect(Object.keys(result.definitions).sort()).toEqual([
    "fragment/MovieFields",
    "operation/MovieFields",
  ]);
  const renderMap = renderMapFromResults([result], {
    projectBase: root,
    ownedRoots: ["src/generated/dsql"],
  });
  expect(renderMap.modules.map((module) => module.id)).toEqual([
    "default/fragment/MovieFields",
    "default/operation/MovieFields",
  ]);
});

test("embedded sources follow daemon targets and legacy ambiguity stays untyped", () => {
  const root = createRoot();
  const hostPath = "embedded.component";
  const querySource = "query MovieInfoLookup { movie_info { id } }";
  const fragmentSource = "fragment MovieFields on movie_info { id }";
  const host = `const query = dsql\`${querySource}\`;\nconst fragment = dsql\`${fragmentSource}\`;\n`;
  const queryStart = host.indexOf(querySource);
  const fragmentStart = host.indexOf(fragmentSource);
  writeFileSync(join(root, hostPath), host);

  const query = {
    ...operationMetadata("MovieInfoLookup"),
    source_map: [
      {
        id: "MovieInfoLookup",
        file: hostPath,
        range: { start: queryStart, end: queryStart + querySource.length },
        content_range: { start: queryStart, end: queryStart + querySource.length },
      },
    ],
  };
  const fragment = {
    ...fragmentMetadata("MovieFields"),
    source_map: [
      {
        id: "MovieFields",
        file: hostPath,
        range: { start: fragmentStart, end: fragmentStart + fragmentSource.length },
        content_range: {
          start: fragmentStart,
          end: fragmentStart + fragmentSource.length,
        },
      },
    ],
  };
  const definitions = [
    { kind: "operation" as const, metadata: query, id: "frontend/operation/MovieInfoLookup" },
    { kind: "fragment" as const, metadata: fragment, id: "frontend/fragment/MovieFields" },
  ];
  const resolved = resolveEmbeddedSources(definitions, {
    projectBase: root,
    callsites: [
      {
        path: hostPath,
        resolver: "typescript",
        contentHash: { algorithm: "sha256", value: sha256Hex(Buffer.from(host)) },
        expressions: [
          { range: { start: 14, end: queryStart + querySource.length + 1 }, target: definitions[0].id },
          {
            range: { start: fragmentStart - 5, end: fragmentStart + fragmentSource.length + 1 },
            target: definitions[1].id,
          },
        ],
      },
    ],
  });
  expect(resolved.mismatches).toEqual([]);
  expect([...resolved.sources]).toEqual([
    ["operation/MovieInfoLookup", querySource],
    ["fragment/MovieFields", fragmentSource],
  ]);

  const ambiguous = resolveEmbeddedSources(
    [
      { kind: "operation", metadata: query },
      {
        kind: "fragment",
        metadata: {
          ...fragment,
          source_map: query.source_map.map((entry) => ({ ...entry, id: "MovieFields" })),
        },
      },
    ],
    { projectBase: root },
  );
  expect([...ambiguous.sources]).toEqual([]);
  expect(ambiguous.mismatches).toEqual([]);
});

test("fragment composition: subtraction, reuse, dedup, and path sensitivity", async () => {
  const root = createRoot();
  // Fragments: Child (on title), Parent (on title, spreads Child at its
  // root), Nested (on kind_type, spread INSIDE a relation elsewhere).
  const child = fragmentMetadata("ChildBits");
  const parent = {
    ...fragmentMetadata("ParentBits"),
    result: {
      fields: [
        // Child-provided...
        ...child.result.fields,
        // ...plus Parent's own addition.
        {
          path: "extra",
          name: "extra",
          parent_path: "",
          kind: "scalar",
          data_type: "text",
          nullable: false,
        },
      ],
    },
    fragment_spreads: [{ path: "", fragment: "ChildBits" }],
  };
  const nested = {
    ...fragmentMetadata("NestedBits"),
    result: {
      fields: [
        {
          path: "kind",
          name: "kind",
          parent_path: "",
          kind: "scalar",
          data_type: "text",
          nullable: false,
        },
      ],
    },
  };

  // The operation selects one relation that spreads Parent (and,
  // transitively recorded by the plan walk, Child) plus its own field,
  // and a second relation whose set spreads Nested plus an extra.
  const operation = {
    ...operationMetadata("Composed"),
    result: {
      fields: [
        {
          path: "movie_info",
          name: "movie_info",
          parent_path: "",
          kind: "array",
          data_type: "object",
          nullable: false,
        },
        // Provided by ParentBits (via ChildBits) at "movie_info".
        {
          path: "movie_info.id",
          name: "id",
          parent_path: "movie_info",
          kind: "scalar",
          data_type: "int",
          nullable: false,
        },
        {
          path: "movie_info.extra",
          name: "extra",
          parent_path: "movie_info",
          kind: "scalar",
          data_type: "text",
          nullable: false,
        },
        // The operation's own addition next to the spreads.
        {
          path: "movie_info.own_field",
          name: "own_field",
          parent_path: "movie_info",
          kind: "scalar",
          data_type: "int",
          nullable: true,
        },
        // A nested relation whose selection set spreads NestedBits and
        // adds a field of its own (the partial-subtree case, valid DSQL:
        // spread + additions inside ONE selection set).
        {
          path: "movie_info.kind",
          name: "kind",
          parent_path: "movie_info",
          kind: "object",
          data_type: "object",
          nullable: false,
        },
        {
          path: "movie_info.kind.kind",
          name: "kind",
          parent_path: "movie_info.kind",
          kind: "scalar",
          data_type: "text",
          nullable: false,
        },
        {
          path: "movie_info.kind.own_kind_field",
          name: "own_kind_field",
          parent_path: "movie_info.kind",
          kind: "scalar",
          data_type: "int",
          nullable: false,
        },
      ],
    },
    fragment_spreads: [
      { path: "movie_info", fragment: "ParentBits" },
      // The plan walk records transitively entered spreads too.
      { path: "movie_info", fragment: "ChildBits" },
      { path: "movie_info.kind", fragment: "NestedBits" },
    ],
  };

  const artifacts = {
    ...createArtifacts(root, { operationNames: ["Composed"] }),
    operations: [operation],
    operationsByName: new Map([[operation.name, operation]]),
    fragments: [child, parent, nested],
    fragmentsByName: new Map([
      ["ChildBits", child],
      ["ParentBits", parent],
      ["NestedBits", nested],
    ]),
  } as BuildArtifacts;

  await renderDsql(artifacts, {
    root,
    queriesDir: "src/generated/dsql/queries",
  });

  const rendered = readFileSync(
    join(root, "src/generated/dsql/queries/Composed.ts"),
    "utf8",
  );
  // One assertion pins the whole composition: Parent covers Child at
  // the same path (dedup), the fragment-provided id/extra/kind.kind
  // are subtracted, and only the definition's own additions stay
  // inline — at the top level and inside the nested selection alike.
  expect(rendered).toContain(
    `export type ComposedResult = {
  movie_info: Array<ParentBitsFragmentResult & {
  own_field: number | null;
  kind: NestedBitsFragmentResult & {
  own_kind_field: number;
};
}>;
};`,
  );
  expect(rendered).not.toContain("ChildBitsFragmentResult");
  // Imports follow the effective set: Parent and Nested, never Child.
  expect(rendered).toContain('import type { ParentBitsFragmentResult } from "./ParentBits.fragment";');
  expect(rendered).toContain('import type { NestedBitsFragmentResult } from "./NestedBits.fragment";');
  expect(rendered).not.toContain("ChildBits.fragment");

  // The composed fragment reuses its parent instead of re-inlining:
  // ParentBits = ChildBitsFragmentResult & { extra } exactly.
  const parentModule = readFileSync(
    join(root, "src/generated/dsql/queries/ParentBits.fragment.ts"),
    "utf8",
  );
  expect(parentModule).toContain(
    'import type { ChildBitsFragmentResult } from "./ChildBits.fragment";',
  );
  expect(parentModule).toContain("ChildBitsFragmentResult & {");
  expect(parentModule).toContain("extra: string;");
  expect(parentModule).not.toMatch(/^\s+id: number;/m);
});

test("nested spreads do not cover the same fragment at other paths", async () => {
  const root = createRoot();
  // Deep spreads Leaf INSIDE a relation; the operation ALSO spreads Leaf
  // directly at the same path it spreads Deep: Deep's nested spread must
  // not swallow the root-level Leaf (path sensitivity of the closure).
  const leaf = fragmentMetadata("LeafBits");
  const deep = {
    ...fragmentMetadata("DeepBits"),
    fragment_spreads: [{ path: "rel", fragment: "LeafBits" }],
  };
  const operation = {
    ...operationMetadata("PathSensitive"),
    fragment_spreads: [
      { path: "movie_info", fragment: "DeepBits" },
      { path: "movie_info", fragment: "LeafBits" },
    ],
  };
  const artifacts = {
    ...createArtifacts(root, { operationNames: ["PathSensitive"] }),
    operations: [operation],
    operationsByName: new Map([[operation.name, operation]]),
    fragments: [leaf, deep],
    fragmentsByName: new Map([
      ["LeafBits", leaf],
      ["DeepBits", deep],
    ]),
  } as BuildArtifacts;

  await renderDsql(artifacts, {
    root,
    queriesDir: "src/generated/dsql/queries",
  });
  const rendered = readFileSync(
    join(root, "src/generated/dsql/queries/PathSensitive.ts"),
    "utf8",
  );
  expect(rendered).toContain("DeepBitsFragmentResult");
  expect(rendered).toContain("LeafBitsFragmentResult");
});

test("rejects generated file-stem collisions", async () => {
  const root = createRoot();
  const artifacts = createArtifacts(root, {
    operationNames: ["movie-info", "movie_info"],
  });

  await expect(
    renderDsql(artifacts, {
      root,
      queriesDir: "src/generated/dsql/queries",
    }),
  ).rejects.toThrow(
    'generated DSQL file-stem collision for MovieInfo: operation "movie-info" and operation "movie_info"',
  );
});

test("does not rewrite unchanged generated files", async () => {
  const root = createRoot();
  const artifacts = createArtifacts(root);

  await renderDsql(artifacts, {
    root,
    queriesDir: "src/generated/dsql/queries",
  });

  const operationPath = join(root, "src/generated/dsql/queries/MovieInfoLookup.ts");
  const before = statSync(operationPath).mtimeMs;
  await Bun.sleep(5);

  await renderDsql(artifacts, {
    root,
    queriesDir: "src/generated/dsql/queries",
  });

  expect(statSync(operationPath).mtimeMs).toBe(before);
});

test("removes stale files recorded in the generated ownership manifest", async () => {
  const root = createRoot();

  await renderDsql(createArtifacts(root), {
    root,
    queriesDir: "src/generated/dsql/queries",
  });
  const operationPath = join(root, "src/generated/dsql/queries/MovieInfoLookup.ts");
  expect(existsSync(operationPath)).toBe(true);

  await renderDsql(createArtifacts(root, { operationNames: [] }), {
    root,
    queriesDir: "src/generated/dsql/queries",
  });

  expect(existsSync(operationPath)).toBe(false);
  expect(
    existsSync(join(root, "src/generated/dsql/queries/MovieFields.fragment.ts")),
  ).toBe(true);
});

function createRoot(): string {
  const root = mkdtempSync(join(tmpdir(), "dsql-render-"));
  const source = movieInfoSource();
  writeFileSync(join(root, "movie-info.dsql"), source);
  return root;
}

function movieInfoSource(): string {
  return "query MovieInfoLookup { movie_info(where .id == $id) { id } }\n";
}

function createArtifacts(
  root: string,
  options: {
    readonly operationNames?: readonly string[];
  } = {},
): BuildArtifacts {
  const operationNames = options.operationNames ?? ["MovieInfoLookup"];
  const operations = operationNames.map((name) => operationMetadata(name));
  const fragments = [fragmentMetadata("MovieFields")];
  return {
    manifestPath: join(root, "dsql/build/manifest.json"),
    currentManifestPath: join(root, "dsql/build/manifest.json"),
    scopes: [{ name: "default", imports: [] }],
    sourceFileScopes: [],
    artifactGroups: [],
    manifest: {
      version: 2,
      generationId: 1,
      operations: operations.map((operation) => ({
        name: operation.name,
        kind: "query",
        path: `operations/${operation.name}.json`,
        hash: `${operation.name}-hash`,
        source: "movie-info.dsql",
      })),
      fragments: fragments.map((fragment) => ({
        name: fragment.name,
        kind: "fragment",
        path: `fragments/${fragment.name}.json`,
        hash: `${fragment.name}-hash`,
        source: "movie-info.dsql",
      })),
    },
    operations,
    operationsByName: new Map(operations.map((operation) => [operation.name, operation])),
    fragments,
    fragmentsByName: new Map(fragments.map((fragment) => [fragment.name, fragment])),
    artifactIds: new Map([
      ...operations.map(
        (operation) =>
          [`operation/${operation.name}`, `default/operation/${operation.name}`] as const,
      ),
      ...fragments.map(
        (fragment) =>
          [`fragment/${fragment.name}`, `default/fragment/${fragment.name}`] as const,
      ),
    ]),
  };
}

function operationMetadata(name: string): BuildArtifacts["operations"][number] {
  return {
    name,
    kind: "query",
    sql: {
      dialect: "postgres",
      text: "select * from movie_info where id = $1 and user_id = $2",
      parameters: [{ path: "params.id" }, { path: "context.user_id" }],
      variants: [],
    },
    result: {
      fields: [
        {
          path: "movie_info",
          name: "movie_info",
          parent_path: "",
          kind: "array",
          data_type: "json",
          nullable: false,
        },
        {
          path: "movie_info.id",
          name: "id",
          parent_path: "movie_info",
          kind: "scalar",
          data_type: "int",
          nullable: false,
        },
      ],
    },
    params: [
      {
        path: "params.id",
        data_type: "int",
        enum_values: [],
        required: true,
        nullable: false,
      },
    ],
    input: [],
    context: [
      {
        path: "context.user_id",
        data_type: "uuid",
        enum_values: [],
        required: true,
        nullable: false,
      },
    ],
    dynamic_inputs: [],
    policies: [],
    handoffs: [],
    fragment_spreads: [],
    source_map: [
      {
        id: name,
        file: "movie-info.dsql",
        range: { start: 0, end: movieInfoSource().trimEnd().length },
      },
    ],
  };
}

function fragmentMetadata(name: string): BuildArtifacts["fragments"][number] {
  return {
    name,
    kind: "fragment",
    table: "movie_info",
    result: {
      fields: [
        {
          path: "id",
          name: "id",
          parent_path: "",
          kind: "scalar",
          data_type: "int",
          nullable: false,
        },
      ],
    },
    params: [],
    input: [],
    dynamic_inputs: [],
    source_map: [],
  };
}
