import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { expect, test } from "bun:test";
import ts from "typescript";
import {
  projectRelative,
  reconcileDsqlOutputs,
  renderDsql as renderDsqlPure,
  renderMapFromResults,
  resolveEmbeddedSources,
  sha256Hex,
  type BuildArtifacts,
  type DsqlRenderResult,
  type RenderDsqlOptions,
} from "../src/node";

async function renderDsql(
  artifacts: BuildArtifacts,
  options: RenderDsqlOptions,
): Promise<DsqlRenderResult> {
  const rendered = await renderDsqlPure(artifacts, options);
  reconcileDsqlOutputs({
    projectBase: options.root,
    ownedRoots: ["src/generated/dsql"],
    files: rendered.files.map((file) => ({
      path: projectRelative(options.root, file.path),
      contents: file.contents,
    })),
  });
  return rendered;
}

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

test("renders readable SQL with compiler-owned escaping and compact SQL on request", async () => {
  const readableRoot = createRoot();
  const readableSql =
    "select `value`, '${binding}', E'path\\\\file'\r\n-- \u2028\u2029\0";
  const compactSql = "select `value`,'${binding}',E'path\\\\file'";
  const operation = {
    ...operationMetadata("SqlPresentation"),
    sql: {
      dialect: "postgres",
      text: readableSql,
      compact_text: compactSql,
      parameters: [],
      variants: [],
    },
  };
  const readableArtifacts = artifactsWithOperation(readableRoot, operation);

  await renderDsql(readableArtifacts, {
    root: readableRoot,
    queriesDir: "src/generated/dsql/queries",
    outputMode: "readable",
  });
  const readableSource = readFileSync(
    join(readableRoot, "src/generated/dsql/queries/SqlPresentation.ts"),
    "utf8",
  );
  expect(readableSource.indexOf("inputs:")).toBeLessThan(
    readableSource.indexOf("sql:"),
  );
  expect(readableSource).toContain("sql: `");
  const readableModule = await evaluateGeneratedModule(readableSource);
  expect(readableModule.SqlPresentationExecutionPayload?.sql).toBe(readableSql);

  const compactRoot = createRoot();
  const compactArtifacts = artifactsWithOperation(compactRoot, operation);
  await renderDsql(compactArtifacts, {
    root: compactRoot,
    queriesDir: "src/generated/dsql/queries",
    outputMode: "compact",
  });
  const compactSource = readFileSync(
    join(compactRoot, "src/generated/dsql/queries/SqlPresentation.ts"),
    "utf8",
  );
  expect(compactSource).toContain(`sql: ${JSON.stringify(compactSql)}`);
  const compactModule = await evaluateGeneratedModule(compactSource);
  expect(compactModule.SqlPresentationExecutionPayload?.sql).toBe(compactSql);
});

test("renders bounded dynamic input types and server execution metadata", async () => {
  const root = createRoot();
  const operation = operationMetadata("SearchUsers");
  operation.params.push(
    {
      path: "params.search",
      data_type: "dynamic_predicate",
      enum_values: [],
      required: false,
      nullable: false,
      default: { kind: "empty_object" },
    },
    {
      path: "params.order",
      data_type: "dynamic_order",
      enum_values: [],
      required: false,
      nullable: false,
      default: { kind: "collection", items: [] },
    },
  );
  operation.dynamic_inputs = [
    {
      path: "params.search",
      kind: "predicate",
      surface: "selected",
      fields: [
        {
          key: "name",
          catalog_path: "public.users.name",
          data_type: "text",
          nullable: false,
          access: "unconditional",
          operators: ["eq", "like", "in", "is_null"],
          directions: [],
        },
      ],
      sites: [],
    },
    {
      path: "params.order",
      kind: "order",
      surface: "selected",
      fields: [
        {
          key: "name",
          catalog_path: "public.users.name",
          data_type: "text",
          nullable: false,
          access: "unconditional",
          operators: [],
          directions: ["asc", "desc_nulls_last"],
        },
      ],
      sites: [],
    },
  ];

  await renderDsql(artifactsWithOperation(root, operation), {
    root,
    queriesDir: "src/generated/dsql/queries",
  });
  const source = readFileSync(
    join(root, "src/generated/dsql/queries/SearchUsers.ts"),
    "utf8",
  );

  expect(source).toContain(
    "export type SearchUsersParamsSearchDynamicInput = {",
  );
  expect(source).toContain("name?: {");
  expect(source).toContain("like?: string;");
  expect(source).toContain("in?: Array<string>;");
  expect(source).toContain(
    'export type SearchUsersParamsOrderDynamicInput = Array<{\n  name: "asc" | "desc_nulls_last";\n}>;',
  );
  expect(source).toContain(
    "search?: SearchUsersParamsSearchDynamicInput;",
  );
  expect(source).toContain("dynamicInputs:");
  expect(source).not.toContain("dynamic_inputs:");
});

test("renders exact numeric, bigint, and finite/non-finite float wire types", async () => {
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
          value_type: resultValueType("object"),
          nullable: false,
        },
        {
          path: "metrics.amount",
          name: "amount",
          parent_path: "metrics",
          kind: "scalar",
          value_type: resultValueType("numeric"),
          nullable: false,
        },
        {
          path: "metrics.ratio",
          name: "ratio",
          parent_path: "metrics",
          kind: "scalar",
          value_type: resultValueType("float"),
          nullable: true,
        },
        {
          path: "metrics.reading_count",
          name: "reading_count",
          parent_path: "metrics",
          kind: "scalar",
          value_type: resultValueType("bigint"),
          nullable: false,
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
      {
        path: "params.minimum_count",
        data_type: "bigint",
        enum_values: [],
        required: true,
        nullable: false,
      },
      {
        path: "params.counts",
        data_type: "bigint",
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
  expect(rendered).toContain("reading_count: string;");
  expect(rendered).toContain(
    'ratio: number | "NaN" | "Infinity" | "-Infinity" | null;',
  );
  expect(rendered).toContain("minimum: string;");
  expect(rendered).toContain(
    'threshold: number | "NaN" | "Infinity" | "-Infinity";',
  );
  expect(rendered).toContain("amounts: Array<string | null>;");
  expect(rendered).toContain("minimum_count: string;");
  expect(rendered).toContain("counts: Array<string | null>;");
});

test("renders database arrays separately from relation and input collections", async () => {
  const root = createRoot();
  const operation = {
    ...operationMetadata("StructuredValues"),
    result: {
      fields: [
        {
          path: "values",
          name: "values",
          parent_path: "",
          kind: "array",
          value_type: resultValueType("object"),
          nullable: false,
        },
        {
          path: "values.labels",
          name: "labels",
          parent_path: "values",
          kind: "scalar",
          value_type: resultValueType("text", "database_array"),
          nullable: false,
        },
        {
          path: "values.big_values",
          name: "big_values",
          parent_path: "values",
          kind: "scalar",
          value_type: resultValueType("bigint", "database_array"),
          nullable: true,
        },
      ],
    },
    params: [
      {
        path: "params.labels",
        data_type: "text",
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
  const source = readFileSync(
    join(root, "src/generated/dsql/queries/StructuredValues.ts"),
    "utf8",
  );
  expect(source).toContain(
    'import type { DsqlDatabaseArray, DsqlExecutionPayload, DsqlOperation } from "@dsql/typescript/runtime";',
  );
  expect(source).toContain("labels: DsqlDatabaseArray<string>;");
  expect(source).toContain("big_values: DsqlDatabaseArray<string> | null;");
  expect(source).toContain("labels: Array<string | null>;");

  const spreadRoot = createRoot();
  const arrayField = {
    path: "labels",
    name: "labels",
    parent_path: "",
    kind: "scalar",
    value_type: resultValueType("text", "database_array"),
    nullable: false,
  };
  const fragment = {
    ...fragmentMetadata("ArrayFields"),
    result: { fields: [arrayField] },
  };
  const spreadOperation = {
    ...operationMetadata("SpreadArrays"),
    result: { fields: [arrayField] },
    fragment_spreads: [{ path: "", fragment: fragment.name }],
  };
  const spreadBase = createArtifacts(spreadRoot, {
    operationNames: [spreadOperation.name],
  });
  const spreadArtifacts = {
    ...spreadBase,
    operations: [spreadOperation],
    operationsByName: new Map([[spreadOperation.name, spreadOperation]]),
    fragments: [fragment],
    fragmentsByName: new Map([[fragment.name, fragment]]),
    artifactIds: new Map([
      [
        `operation/${spreadOperation.name}`,
        `default/operation/${spreadOperation.name}`,
      ],
      [`fragment/${fragment.name}`, `default/fragment/${fragment.name}`],
    ]),
  } as BuildArtifacts;
  await renderDsql(spreadArtifacts, {
    root: spreadRoot,
    queriesDir: "src/generated/dsql/queries",
  });
  const spreadSource = readFileSync(
    join(spreadRoot, "src/generated/dsql/queries/SpreadArrays.ts"),
    "utf8",
  );
  expect(spreadSource).toContain(
    'import type { ArrayFieldsFragmentResult } from "./ArrayFields.fragment";',
  );
  expect(spreadSource).toContain(
    'import type { DsqlExecutionPayload, DsqlOperation } from "@dsql/typescript/runtime";',
  );
  expect(spreadSource).not.toContain("DsqlDatabaseArray");
});

test("renders defaulted and nullable inputs as optional TypeScript properties", async () => {
  const root = createRoot();
  const operation = {
    ...operationMetadata("OptionalInputs"),
    sql: {
      dialect: "postgres",
      text: "select * from movie_info limit $1",
      compact_text: "select * from movie_info limit $1",
      parameters: [{ path: "params.limit" }],
      variants: [],
    },
    params: [
      {
        path: "params.limit",
        data_type: "int",
        enum_values: [],
        required: false,
        nullable: true,
        default: { kind: "null" },
      },
    ],
    input: [
      {
        path: "input.search.clause.minimum",
        data_type: "int",
        enum_values: [],
        required: false,
        nullable: false,
        default: { kind: "number", value: "10" },
      },
    ],
    context: [],
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
    join(root, "src/generated/dsql/queries/OptionalInputs.ts"),
    "utf8",
  );

  expect(rendered).toContain("limit?: number | null;");
  expect(rendered).toContain("search?: {");
  expect(rendered).toContain("minimum?: number;");
  expect(rendered).toContain('"default":{"kind":"null"}');
  expect(rendered).toContain('"default":{"kind":"number","value":"10"}');
});

test("renders policy-nullable singular and flattened results while runtime limits stay arrays", async () => {
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
          value_type: resultValueType("object"),
          nullable: true,
        },
        {
          path: "singular.id",
          name: "id",
          parent_path: "singular",
          kind: "scalar",
          value_type: resultValueType("int"),
          nullable: false,
        },
        {
          path: "runtime",
          name: "runtime",
          parent_path: "",
          kind: "array",
          value_type: resultValueType("object"),
          nullable: false,
        },
        {
          path: "runtime.id",
          name: "id",
          parent_path: "runtime",
          kind: "scalar",
          value_type: resultValueType("int"),
          nullable: false,
        },
        {
          path: "owner_name",
          name: "owner_name",
          parent_path: "",
          kind: "scalar",
          value_type: resultValueType("text"),
          nullable: true,
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
  expect(rendered).toContain("owner_name: string | null;");
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

test("embedded sources follow daemon targets and ambiguous mappings stay untyped", () => {
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
  expect(() =>
    resolveEmbeddedSources(definitions, {
      projectBase: root,
      callsites: [
        {
          path: hostPath,
          resolver: "typescript",
          contentHash: { algorithm: "sha256", value: sha256Hex(Buffer.from(host)) },
          expressions: [
            {
              range: { start: 14, end: queryStart + querySource.length + 1 },
              target: "frontend/operation/Missing",
            },
          ],
        },
      ],
    }),
  ).toThrow(
    'dsql compile result references missing artifact "frontend/operation/Missing" ' +
      'for "embedded.component"',
  );

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
          value_type: resultValueType("text"),
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
          value_type: resultValueType("text"),
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
          value_type: resultValueType("object"),
          nullable: false,
        },
        // Provided by ParentBits (via ChildBits) at "movie_info".
        {
          path: "movie_info.id",
          name: "id",
          parent_path: "movie_info",
          kind: "scalar",
          value_type: resultValueType("int"),
          nullable: false,
        },
        {
          path: "movie_info.extra",
          name: "extra",
          parent_path: "movie_info",
          kind: "scalar",
          value_type: resultValueType("text"),
          nullable: false,
        },
        // The operation's own addition next to the spreads.
        {
          path: "movie_info.own_field",
          name: "own_field",
          parent_path: "movie_info",
          kind: "scalar",
          value_type: resultValueType("int"),
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
          value_type: resultValueType("object"),
          nullable: false,
        },
        {
          path: "movie_info.kind.kind",
          name: "kind",
          parent_path: "movie_info.kind",
          kind: "scalar",
          value_type: resultValueType("text"),
          nullable: false,
        },
        {
          path: "movie_info.kind.own_kind_field",
          name: "own_kind_field",
          parent_path: "movie_info.kind",
          kind: "scalar",
          value_type: resultValueType("int"),
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

test("rendering stays pure until the desired files are published", async () => {
  const root = createRoot();
  const rendered = await renderDsqlPure(createArtifacts(root), {
    root,
    queriesDir: "src/generated/dsql/queries",
  });
  const operationPath = join(root, "src/generated/dsql/queries/MovieInfoLookup.ts");

  expect(existsSync(operationPath)).toBe(false);
  expect(
    rendered.files.some((file) => file.path === operationPath),
  ).toBe(true);
});

test("reconciles generated files without persistent ownership state", async () => {
  const root = createRoot();

  await renderDsql(createArtifacts(root), {
    root,
    queriesDir: "src/generated/dsql/queries",
  });
  const operationPath = join(root, "src/generated/dsql/queries/MovieInfoLookup.ts");
  expect(existsSync(operationPath)).toBe(true);

  const nestedStale = join(root, "src/generated/dsql/queries/stale/StaleQuery.ts");
  mkdirSync(join(root, "src/generated/dsql/queries/stale"), { recursive: true });
  writeFileSync(nestedStale, "export {};\n");

  const rendered = await renderDsql(createArtifacts(root, { operationNames: [] }), {
    root,
    queriesDir: "src/generated/dsql/queries",
  });
  reconcileDsqlOutputs({
    projectBase: root,
    ownedRoots: ["src/generated/dsql"],
    files: rendered.files.map((file) => ({
      path: projectRelative(root, file.path),
      contents: file.contents,
    })),
  });

  expect(existsSync(operationPath)).toBe(false);
  expect(existsSync(nestedStale)).toBe(false);
  expect(existsSync(join(root, "src/generated/dsql/queries/stale"))).toBe(false);
  expect(
    existsSync(join(root, "src/generated/dsql/queries/MovieFields.fragment.ts")),
  ).toBe(true);
});

test("rejects unsafe owned roots before reconciling", () => {
  const root = createRoot();
  const authored = join(root, "authored.ts");
  writeFileSync(authored, "export {};\n");

  expect(() =>
    reconcileDsqlOutputs({
      projectBase: root,
      ownedRoots: ["."],
      files: [],
    }),
  ).toThrow("is not a plain project-base-relative path");
  expect(existsSync(authored)).toBe(true);
});

test("does not follow a symlinked owned root", () => {
  const root = createRoot();
  const target = join(root, "generated-target");
  const authored = join(target, "authored.ts");
  mkdirSync(target);
  writeFileSync(authored, "export {};\n");
  symlinkSync(target, join(root, "generated"), "dir");

  expect(() =>
    reconcileDsqlOutputs({
      projectBase: root,
      ownedRoots: ["generated"],
      files: [],
    }),
  ).toThrow("is not a directory");
  expect(existsSync(authored)).toBe(true);
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
      version: 6,
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

function artifactsWithOperation(
  root: string,
  operation: BuildArtifacts["operations"][number],
): BuildArtifacts {
  const artifacts = createArtifacts(root, { operationNames: [operation.name] });
  return {
    ...artifacts,
    operations: [operation],
    operationsByName: new Map([[operation.name, operation]]),
  };
}

async function evaluateGeneratedModule(
  source: string,
): Promise<Record<string, { readonly sql?: string } | undefined>> {
  const output = ts.transpileModule(source, {
    compilerOptions: {
      module: ts.ModuleKind.ESNext,
      target: ts.ScriptTarget.ES2022,
    },
  }).outputText;
  const encoded = Buffer.from(output, "utf8").toString("base64");
  return (await import(`data:text/javascript;base64,${encoded}`)) as Record<
    string,
    { readonly sql?: string } | undefined
  >;
}

function operationMetadata(name: string): BuildArtifacts["operations"][number] {
  return {
    name,
    kind: "query",
    sql: {
      dialect: "postgres",
      text: "select * from movie_info where id = $1 and user_id = $2",
      compact_text: "select * from movie_info where id = $1 and user_id = $2",
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
          value_type: resultValueType("object"),
          nullable: false,
        },
        {
          path: "movie_info.id",
          name: "id",
          parent_path: "movie_info",
          kind: "scalar",
          value_type: resultValueType("int"),
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
          value_type: resultValueType("int"),
          nullable: false,
        },
      ],
    },
    params: [],
    input: [],
    dynamic_inputs: [],
    fragment_spreads: [],
    source_map: [],
  };
}

function resultValueType(
  name: "object" | "boolean" | "int" | "bigint" | "float" | "numeric" | "json" | "text" | "timestamptz" | "uuid",
  shape: "scalar" | "database_array" | "object" =
    name === "object" ? "object" : "scalar",
) {
  const encodings = {
    object: "unsupported",
    boolean: "boolean",
    int: "integer",
    bigint: "big_integer",
    float: "float",
    numeric: "numeric",
    json: "json",
    text: "text",
    timestamptz: "timestamptz",
    uuid: "uuid",
  } as const;
  return {
    shape,
    name,
    wire: { encoding: encodings[name] },
  };
}
