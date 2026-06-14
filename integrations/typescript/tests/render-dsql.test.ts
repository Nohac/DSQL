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
import { renderDsql, type BuildArtifacts } from "../src/node";

test("renders inline per-definition dsql modules", async () => {
  const root = createRoot();
  const artifacts = createArtifacts(root);

  const result = await renderDsql(artifacts, {
    root,
    queriesDir: "src/generated/dsql/queries",
  });

  expect(result.modules.queries).toBe("./src/generated/dsql/queries/index");
  expect(result.definitions.MovieInfoLookup).toEqual({
    name: "MovieInfoLookup",
    kind: "query",
    operationModule: "./src/generated/dsql/queries/MovieInfoLookup",
    executionModule: "./src/generated/dsql/queries/MovieInfoLookup",
  });

  const operation = readFileSync(
    join(root, "src/generated/dsql/queries/MovieInfoLookup.ts"),
    "utf8",
  );
  expect(operation).toContain("export type MovieInfoLookupResult");
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

  const index = readFileSync(
    join(root, "src/generated/dsql/queries/index.ts"),
    "utf8",
  );
  expect(index).toBe(
    'export { dsql } from "@dsql/typescript/runtime";\nexport type { DsqlDefinition, DsqlExecutionPayload, DsqlFragment, DsqlFragmentDefinition, DsqlFragmentInput, DsqlFragmentParams, DsqlFragmentVariables, DsqlMaterializedQuery, DsqlOperation, DsqlOperationInput, DsqlOperationParams, DsqlOperationResult, DsqlVariables } from "@dsql/typescript/runtime";\nexport * from "./MovieFields.fragment";\nexport * from "./MovieInfoLookup";\n',
  );
});

test("renders split query and execution modules with matching filenames", async () => {
  const root = createRoot();
  const artifacts = createArtifacts(root);

  const result = await renderDsql(artifacts, {
    root,
    queriesDir: "src/generated/dsql/queries",
    executionDir: "src/generated/dsql/queries.server",
  });

  expect(result.definitions.MovieInfoLookup?.executionModule).toBe(
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
    scopes: [{ name: "default", imports: [] }],
    sourceFileScopes: [],
    artifactGroups: [],
    manifest: {
      version: 1,
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
  };
}

function operationMetadata(name: string): BuildArtifacts["operations"][number] {
  return {
    name,
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
    context: [],
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
