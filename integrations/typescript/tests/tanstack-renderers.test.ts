import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "bun:test";
import {
  ModuleKind,
  ModuleResolutionKind,
  Project,
  ScriptTarget,
} from "ts-morph";
import {
  projectRelative,
  reconcileDsqlOutputs,
  renderDsql,
  type BuildArtifacts,
  type DsqlDesiredFile,
} from "../src/node";
import type { DsqlProjectGeneratorContext } from "../src/renderer";
import { tanstackQuery } from "../renderers/generators/tanstack-query";
import { tanstackStart } from "../renderers/generators/tanstack-start";

test("tanstack start uses imported validator expressions when provided", async () => {
  const root = createRoot();
  const artifacts = createArtifacts(root);
  const dsql = await renderDsql(artifacts, {
    root,
    queriesDir: "src/generated/dsql/queries",
    executionDir: "src/generated/dsql/queries.server",
  });

  const files = new Map<string, string>(
    dsql.files.map((file) => [projectRelative(root, file.path), file.contents]),
  );
  const context: DsqlProjectGeneratorContext = {
    target: "default",
    projectBase: root,
    outputDirectory: "src/generated/dsql",
    artifacts,
    embeddedSources: new Map(),
    files: {
      write(path, contents) {
        files.set(join("src/generated/dsql", path), contents);
      },
    },
    definitions: { current: dsql },
    mode: "test",
    command: "build",
    outputMode: "readable",
    scalars: {},
  };
  await tanstackStart({
    validatorFor(operation) {
      if (operation.name !== "MovieInfoLookup") {
        return "identity";
      }

      return {
        import: {
          name: "MovieInfoVariablesSchema",
          from: "@/validation/movie-info",
        },
        expression: "MovieInfoVariablesSchema.parse",
      };
    },
  }).render(context);
  await tanstackQuery().render(context);
  reconcileDsqlOutputs({
    projectBase: root,
    ownedRoots: ["src/generated/dsql"],
    files: [...files].map(
      ([path, contents]): DsqlDesiredFile => ({
        path,
        contents,
      }),
    ),
  });

  const source = readFileSync(
    join(root, "src/generated/dsql/tanstack-start.ts"),
    "utf8",
  );
  expect(source).toContain(
    'import { MovieInfoVariablesSchema } from "@/validation/movie-info";',
  );
  expect(source).toContain(
    ".inputValidator(MovieInfoVariablesSchema.parse as (variables: DsqlServerVariables<typeof MovieInfoLookupOperation>) => DsqlServerVariables<typeof MovieInfoLookupOperation>)",
  );
  expect(source).toContain("readonly provideContext?: DsqlContextProvider;");
  expect(source).toContain(
    "await context.dsql.provideContext({ operation, variables })",
  );
  expect(source).toContain("DsqlWireVariables<Operation>");
  expect(source).toContain("Promise<DsqlOperationWireResult<Operation>>");
  expect(source).toContain("context: materialized.context");
  const querySource = readFileSync(
    join(root, "src/generated/dsql/tanstack-query.ts"),
    "utf8",
  );
  expect(querySource).toContain(
    '"MovieInfoLookup": MovieInfoLookupServerFn as DsqlServerFunction<typeof MovieInfoLookupOperation>',
  );
  expect(querySource).toContain("readonly contextScope: string");
  expect(querySource).toContain(
    "queryKey: dsqlQueryKeyForWire(",
  );
  expect(querySource).not.toContain("data: { contextScope");
  const operationSource = readFileSync(
    join(root, "src/generated/dsql/queries/MovieInfoLookup.ts"),
    "utf8",
  );
  expect(operationSource).toContain("requiresContext: true");
  expect(operationSource).toContain(
    'inputs: [{"path":"params.id","data_type":"int","wire":{"encoding":"integer"},"validation":{},"enum_values":[],"required":false,"nullable":false,"default":{"kind":"number","value":"7"}}]',
  );
  expect(operationSource).not.toContain("context.tenant_id");
  expect(operationSource).not.toContain("select * from movie_info");

  assertGeneratedQueryConsumerTypes(root, querySource);
});

function assertGeneratedQueryConsumerTypes(
  root: string,
  querySource: string,
): void {
  const generatedDirectory = join(root, "src/generated/dsql");
  writeFileSync(
    join(root, "runtime.ts"),
    readFileSync(new URL("../src/runtime.ts", import.meta.url), "utf8"),
  );
  mkdirSync(join(root, "generated"));
  writeFileSync(
    join(root, "generated/metadata.ts"),
    readFileSync(
      new URL("../src/generated/metadata.ts", import.meta.url),
      "utf8",
    ),
  );
  writeFileSync(
    join(root, "react-query.d.ts"),
    `
export type UseQueryOptions<Result, ErrorType, Selected, Key> = {};
export type UseQueryResult<Result, ErrorType> = {
  readonly data: Result | undefined;
};
export function queryOptions<Options>(options: Options): Options;
export function useQuery<Options>(options: Options): unknown;
`,
  );
  writeFileSync(
    join(generatedDirectory, "queries/index.ts"),
    `
import type { DsqlOperation } from "@dsql/typescript/runtime";
export const MovieInfoLookupOperation =
  {} as DsqlOperation<unknown, Record<string, never>, Record<string, never>, Record<string, never>>;
`,
  );
  writeFileSync(
    join(generatedDirectory, "tanstack-start.ts"),
    `
import type { DsqlOperation, DsqlWireVariables } from "@dsql/typescript/runtime";
export type DsqlServerVariables<
  Operation extends DsqlOperation<any, any, any, any, any>,
> = DsqlWireVariables<Operation>;
export const MovieInfoLookupServerFn = async () => undefined;
`,
  );
  writeFileSync(join(generatedDirectory, "tanstack-query.ts"), querySource);
  writeFileSync(
    join(root, "consumer.ts"),
    `
import type { DsqlOperation } from "@dsql/typescript/runtime";
import { executeQuery, useQuery } from "./src/generated/dsql/tanstack-query";

declare const optionalOperation: DsqlOperation<
  unknown,
  { info?: string | null },
  { search?: { term?: string | null } },
  Record<string, never>
>;
useQuery(optionalOperation);
useQuery(optionalOperation, {});
useQuery(optionalOperation, { params: {} });
useQuery(optionalOperation, { input: {} });
executeQuery(optionalOperation);

declare const requiredNullableOperation: DsqlOperation<
  unknown,
  { info: string | null },
  Record<string, never>,
  Record<string, never>
>;
// @ts-expect-error nullable without a default remains required
useQuery(requiredNullableOperation);
useQuery(requiredNullableOperation, { params: { info: null } });
executeQuery(requiredNullableOperation, { params: { info: null } });

declare const requiredOperation: DsqlOperation<
  unknown,
  { id: number },
  Record<string, never>,
  Record<string, never>
>;
// @ts-expect-error required public variables keep the options argument required
useQuery(requiredOperation);
useQuery(requiredOperation, { params: { id: 1 } });

declare const contextOperation: DsqlOperation<
  unknown,
  { info?: string | null },
  Record<string, never>,
  { tenant_id?: string }
>;
// @ts-expect-error trusted context keeps the options argument required
useQuery(contextOperation);
// @ts-expect-error trusted context requires an explicit cache scope
useQuery(contextOperation, {});
useQuery(contextOperation, { contextScope: "tenant" });

type DateValue = { readonly iso: string };
declare const mappedOperation: DsqlOperation<
  { readonly released: DateValue },
  { readonly since: DateValue },
  Record<string, never>,
  Record<string, never>,
  import("@dsql/typescript/runtime").DsqlWireContract<
    { readonly released: string },
    { readonly since: string },
    Record<string, never>
  >
>;
const selected = useQuery(mappedOperation, {
  params: { since: { iso: "2026-01-01" } },
  select: (result) => result.released.iso,
});
const selectedData: string | undefined = selected.data;
// @ts-expect-error public callers pass host values, never wire values
useQuery(mappedOperation, { params: { since: "2026-01-01" } });
const executed: Promise<{ readonly released: DateValue }> = executeQuery(
  mappedOperation,
  { params: { since: { iso: "2026-01-01" } } },
);
`,
  );

  const project = new Project({
    compilerOptions: {
      baseUrl: root,
      exactOptionalPropertyTypes: true,
      module: ModuleKind.ESNext,
      moduleResolution: ModuleResolutionKind.Bundler,
      paths: {
        "@dsql/typescript/runtime": ["runtime.ts"],
        "@tanstack/react-query": ["react-query.d.ts"],
      },
      strict: true,
      target: ScriptTarget.ES2022,
    },
    skipAddingFilesFromTsConfig: true,
  });
  project.addSourceFileAtPath(join(root, "consumer.ts"));
  expect(
    project
      .getPreEmitDiagnostics()
      .map((diagnostic) => diagnostic.getMessageText()),
  ).toEqual([]);
}

function createRoot(): string {
  const root = mkdtempSync(join(tmpdir(), "dsql-tanstack-"));
  writeFileSync(
    join(root, "movie-info.dsql"),
    "query MovieInfoLookup { movie_info(where .id == $id) { id } }\n",
  );
  return root;
}

function createArtifacts(root: string): BuildArtifacts {
  const operation = operationMetadata();
  return {
    manifestPath: join(root, "dsql/build/manifest.json"),
    currentManifestPath: join(root, "dsql/build/manifest.json"),
    scopes: [{ name: "default", imports: [] }],
    sourceFileScopes: [],
    artifactGroups: [],
    manifest: {
      version: 6,
      generationId: 1,
      operations: [
        {
          name: operation.name,
          kind: "query",
          path: "operations/MovieInfoLookup.json",
          hash: "movie-info-hash",
          source: "movie-info.dsql",
        },
      ],
      fragments: [],
    },
    operations: [operation],
    operationsByName: new Map([[operation.name, operation]]),
    fragments: [],
    fragmentsByName: new Map(),
    artifactIds: new Map([
      ["operation/MovieInfoLookup", "default/operation/MovieInfoLookup"],
    ]),
  };
}

function operationMetadata(): BuildArtifacts["operations"][number] {
  return {
    name: "MovieInfoLookup",
    kind: "query",
    sql: {
      dialect: "postgres",
      text: "select * from movie_info where id = $1",
      compact_text: "select * from movie_info where id = $1",
      parameters: [{ path: "params.id" }],
      variants: [],
    },
    result: {
      fields: [
        {
          path: "movie_info",
          name: "movie_info",
          parent_path: "",
          kind: "array",
          value_type: {
            shape: "object",
            name: "object",
            wire: { encoding: "unsupported" },
          },
          nullable: false,
          access: "unconditional",
        },
      ],
    },
    params: [
      {
        path: "params.id",
        data_type: "int",
        wire: { encoding: "integer" },
        validation: {},
        enum_values: [],
        required: false,
        nullable: false,
        default: { kind: "number", value: "7" },
      },
    ],
    input: [],
    context: [
      {
        path: "context.tenant_id",
        data_type: "uuid",
        wire: { encoding: "uuid" },
        validation: {},
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
        id: "MovieInfoLookup",
        file: "movie-info.dsql",
        range: { start: 0, end: 60 },
      },
    ],
  };
}
