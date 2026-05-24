import { expect, test } from "bun:test";
import { transformDsqlTags } from "../src/vite";
import { buildArtifactsFromGenerated, defineDsqlGenerator } from "../src/node";

test("transforms named dsql query tags into generated operation imports", () => {
  const result = transformDsqlTags(
    `import { dsql } from "./generated/dsql/queries";

const MovieInfo = dsql\`
  query MovieInfoLookup {
    movie_info {
      id
    }
  }
\`;
`,
    "./generated/dsql/queries",
  );

  expect(result).toEqual({
    code: `import { MovieInfoLookupOperation as MovieInfo } from "./generated/dsql/queries";
import { dsql } from "./generated/dsql/queries";


`,
    map: null,
  });
});

test("transforms named dsql function calls into generated operation imports", () => {
  const result = transformDsqlTags(
    `import { dsql } from "./generated/dsql/queries";

const MovieInfo = dsql(\`
  query MovieInfoLookup {
    movie_info {
      id
    }
  }
\`);
`,
    "./generated/dsql/queries",
  );

  expect(result).toEqual({
    code: `import { MovieInfoLookupOperation as MovieInfo } from "./generated/dsql/queries";
import { dsql } from "./generated/dsql/queries";


`,
    map: null,
  });
});

test("preserves exported dsql query bindings", () => {
  const result = transformDsqlTags(
    `export const Users = dsql\`
  query Users {
    users {
      id
    }
  }
\`;
`,
    "/src/generated/dsql/queries",
  );

  expect(result).toEqual({
    code: `import { UsersOperation as Users } from "/src/generated/dsql/queries";
export { Users };`,
    map: null,
  });
});

test("leaves fragment-only dsql bindings untransformed", () => {
  const code = `import { dsql } from "./generated/dsql";

const MovieCompany = dsql(\`
fragment MovieCompany on movie_companies {
  note
  title {
    id
    title
  }
}
\`);
`;

  expect(transformDsqlTags(code, "./generated/dsql/queries")).toBe(null);
});

test("rejects JavaScript interpolation in dsql tags", () => {
  expect(() =>
    transformDsqlTags("const Users = dsql`query Users { users(where .id == ${id}) { id } }`;"),
  ).toThrow("dsql templates do not support JavaScript interpolation");
});

test("builds generator artifacts from daemon output", async () => {
  const artifacts = buildArtifactsFromGenerated({
    project_dir: "/project",
    out_dir: "/project/src/generated/dsql",
    manifest_path: "/project/dsql/build/manifest.json",
    manifest: {
      version: 1,
      operations: [
        {
          name: "MovieInfo",
          kind: "query",
          path: "operations/MovieInfo.json",
          hash: "hash",
          source: "queries/movie-info.dsql",
        },
      ],
    },
    operations: [
      {
        hash: "hash",
        source: "queries/movie-info.dsql",
        metadata: {
          name: "MovieInfo",
          kind: "query",
          sql: {
            dialect: "postgres",
            text: "select 1",
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
          source_map: [],
        },
      },
    ],
  });

  const generator = defineDsqlGenerator(({ artifacts }) => {
    expect(artifacts.operationsByName.get("MovieInfo")?.sql.text).toBe(
      "select 1",
    );
  });

  await generator({
    artifacts,
    root: "/project",
    outDir: "/project/src/generated/dsql",
    mode: "test",
    command: "serve",
  });
});
