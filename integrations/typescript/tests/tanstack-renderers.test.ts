import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "bun:test";
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
  const querySource = readFileSync(
    join(root, "src/generated/dsql/tanstack-query.ts"),
    "utf8",
  );
  expect(querySource).toContain(
    '"MovieInfoLookup": MovieInfoLookupServerFn as DsqlServerFunction<typeof MovieInfoLookupOperation>',
  );
  expect(querySource).toContain("readonly contextScope: string");
  expect(querySource).toContain(
    "queryKey: queryKey(operation, variables, contextScope)",
  );
  expect(querySource).not.toContain("data: { contextScope");
  const operationSource = readFileSync(
    join(root, "src/generated/dsql/queries/MovieInfoLookup.ts"),
    "utf8",
  );
  expect(operationSource).toContain("requiresContext: true");
  expect(operationSource).toContain(
    'inputs: [{"path":"params.id","data_type":"int","enum_values":[],"required":false,"nullable":false,"default":{"kind":"number","value":"7"}}]',
  );
  expect(operationSource).not.toContain("context.tenant_id");
  expect(operationSource).not.toContain("select * from movie_info");
});

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
      version: 2,
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
          data_type: "json",
          nullable: false,
          access: "unconditional",
        },
      ],
    },
    params: [
      {
        path: "params.id",
        data_type: "int",
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
