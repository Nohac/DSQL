import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "bun:test";
import { renderDsql, type BuildArtifacts } from "../src/node";
import { renderTanStackStart } from "../renderers/generators/tanstack-start";

test("tanstack start uses imported validator expressions when provided", async () => {
  const root = createRoot();
  const artifacts = createArtifacts(root);
  const dsql = await renderDsql(artifacts, {
    root,
    queriesDir: "src/generated/dsql/queries",
    executionDir: "src/generated/dsql/queries.server",
  });

  await renderTanStackStart(artifacts, dsql, {
    root,
    outDir: join(root, "src/generated/dsql"),
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
    scopes: [{ name: "default", imports: [] }],
    sourceFileScopes: [],
    manifest: {
      version: 1,
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
    context: [],
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
