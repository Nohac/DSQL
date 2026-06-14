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

test("transforms named dsql fragment calls into generated fragment imports", () => {
  const result = transformDsqlTags(
    `import { dsql } from "./generated/dsql";

const MovieCompany = dsql(\`
fragment MovieCompany on movie_companies {
  note
  company_type {
    kind
  }
}
\`);
`,
    "./generated/dsql/queries",
  );

  expect(result).toEqual({
    code: `import { MovieCompanyFragment as MovieCompany } from "./generated/dsql/queries";
import { dsql } from "./generated/dsql";


`,
    map: null,
  });
});

test("preserves exported dsql fragment bindings", () => {
  const result = transformDsqlTags(
    `export const MovieCompany = dsql\`
fragment MovieCompany on movie_companies {
  note
}
\`;
`,
    "/src/generated/dsql/queries",
  );

  expect(result).toEqual({
    code: `import { MovieCompanyFragment as MovieCompany } from "/src/generated/dsql/queries";
export { MovieCompany };`,
    map: null,
  });
});

test("leaves unnamed fragment-only dsql bindings untransformed", () => {
  const code = `import { dsql } from "./generated/dsql";

const MovieCompany = dsql(\`
fragment on movie_companies {
  note
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
    manifest_path: "/project/dsql/build/manifest.json",
    scopes: [{ name: "default", imports: [] }],
    source_file_scopes: [
      {
        file: "/project/queries/movie-info.dsql",
        source_offset: 0,
        scope: "default",
      },
    ],
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
      fragments: [],
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
          fragment_spreads: [],
          source_map: [],
        },
      },
    ],
    fragments: [],
  });

  const generator = defineDsqlGenerator(({ artifacts }) => {
    expect(artifacts.operationsByName.get("MovieInfo")?.sql.text).toBe(
      "select 1",
    );
    expect(artifacts.scopes[0]?.name).toBe("default");
  });

  await generator({
    artifacts,
    root: "/project",
    mode: "test",
    command: "serve",
  });
});

test("generators may return dsql render metadata", async () => {
  const artifacts = buildArtifactsFromGenerated({
    project_dir: "/project",
    manifest_path: "/project/dsql/build/manifest.json",
    scopes: [{ name: "frontend", imports: ["shared"] }],
    source_file_scopes: [],
    manifest: {
      version: 1,
      operations: [],
      fragments: [],
    },
    operations: [],
    fragments: [],
  });

  const generator = defineDsqlGenerator(() => ({
    modules: {
      queries: "./src/generated/dsql/queries/index",
    },
    definitions: {},
    files: [],
  }));

  const result = await generator({
    artifacts,
    root: "/project",
    mode: "test",
    command: "serve",
  });

  expect(Array.isArray(result) ? result[0]?.modules.queries : result?.modules.queries).toBe(
    "./src/generated/dsql/queries/index",
  );
});
