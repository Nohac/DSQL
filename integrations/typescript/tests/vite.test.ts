import { expect, test } from "bun:test";
import {
  ignoreDsqlRenderedOutput,
  isDsqlRelevantFile,
  isRenderedDsqlOutputFile,
  renderedOutputWatchIgnored,
  renderedOutputDirectories,
  transformDsqlTags,
  type ViteWatchIgnored,
} from "../src/vite";
import { buildArtifactsFromGenerated, defineDsqlGenerator } from "../src/node";
import type { DsqlRenderResult } from "../src/render/types";

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
    `import { dsql } from "./generated/dsql/queries";

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
import { dsql } from "./generated/dsql/queries";


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
  const code = `import { dsql } from "./generated/dsql/queries";

const MovieCompany = dsql(\`
fragment on movie_companies {
  note
}
\`);
`;

  expect(transformDsqlTags(code, "./generated/dsql/queries")).toBe(null);
});

test("does not require render metadata for unrelated dsql identifiers", () => {
  const code = `export const context = {
  dsql: {
    executeQuery() {
      return undefined;
    },
  },
};
`;

  expect(
    transformDsqlTags(code, () => {
      throw new Error("metadata should not be resolved");
    }),
  ).toBe(null);
});

test("rejects JavaScript interpolation in dsql tags", () => {
  expect(() =>
    transformDsqlTags(
      "const Users = dsql`query Users { users(where .id == ${id}) { id } }`;",
      "./generated/dsql/queries",
    ),
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

test("generated dsql output files are not hmr compile triggers", () => {
  const renderResults = [
    renderResult([
      "/project/src/generated/dsql/queries/MovieInfoLookup.ts",
      "/project/src/generated/dsql/queries/index.ts",
      "/project/src/generated/dsql/queries.server/MovieInfoLookup.ts",
    ]),
  ];
  const sourceFileScopes = [
    { file: "/project/src/routes/movie.tsx", scope: "frontend" },
  ];

  expect(
    renderedOutputDirectories(renderResults),
  ).toEqual([
    "/project/src/generated/dsql/queries",
    "/project/src/generated/dsql/queries.server",
  ]);
  expect(
    isRenderedDsqlOutputFile(
      "/project/src/generated/dsql/queries/MovieInfoLookup.ts",
      renderResults,
    ),
  ).toBe(true);
  expect(
    isRenderedDsqlOutputFile(
      "/project/src/generated/dsql/queries/OtherNewFile.ts",
      renderResults,
    ),
  ).toBe(true);
  expect(
    isDsqlRelevantFile(
      "/project/src/generated/dsql/queries/MovieInfoLookup.ts",
      renderResults,
      sourceFileScopes,
    ),
  ).toBe(false);
  expect(
    isDsqlRelevantFile(
      "/project/src/routes/movie.tsx",
      renderResults,
      sourceFileScopes,
    ),
  ).toBe(true);
  expect(
    isDsqlRelevantFile(
      "/project/src/routes/unrelated.tsx",
      renderResults,
      sourceFileScopes,
    ),
  ).toBe(false);
  expect(
    isDsqlRelevantFile(
      "/project/queries/movie.dsql",
      renderResults,
      sourceFileScopes,
    ),
  ).toBe(true);
});

test("vite watcher ignores generated dsql output directories", () => {
  const renderResults = [
    renderResult([
      "/project/src/generated/dsql/queries/MovieInfoLookup.ts",
      "/project/src/generated/dsql/queries.server/MovieInfoLookup.ts",
    ]),
  ];
  const unwatched: string[] = [];
  const options = Object.freeze({ ignored: ["**/node_modules/**"] });
  const watcher = {
    options,
    unwatch(paths: string | readonly string[]) {
      unwatched.push(...(Array.isArray(paths) ? paths : [paths]));
    },
  };

  ignoreDsqlRenderedOutput({ watcher }, renderResults);

  expect(unwatched).toEqual([
    "/project/src/generated/dsql/queries",
    "/project/src/generated/dsql/queries.server",
  ]);
  expect(watcher.options).toBe(options);
});

test("vite watch ignores are derived from generated render metadata", () => {
  const renderResults = [
    renderResult([
      "/project/app/dsql-output/queries/MovieInfoLookup.ts",
      "/external/generated/dsql/index.ts",
    ]),
  ];
  const ignored = renderedOutputWatchIgnored(/node_modules/, renderResults);

  expect(
    matchesIgnored(ignored, "/project/app/dsql-output/queries/index.ts"),
  ).toBe(true);
  expect(matchesIgnored(ignored, "/external/generated/dsql/index.ts")).toBe(true);
  expect(matchesIgnored(ignored, "/project/node_modules/pkg/index.js")).toBe(true);
  expect(matchesIgnored(ignored, "/project/src/routes/index.tsx")).toBe(false);
});

function renderResult(files: readonly string[]): DsqlRenderResult {
  return {
    modules: {
      queries: "./src/generated/dsql/queries/index",
    },
    definitions: {},
    files: files.map((path) => ({ path, contents: "" })),
  };
}

function matchesIgnored(
  ignored: ViteWatchIgnored | undefined,
  path: string,
): boolean {
  if (!ignored) {
    return false;
  }
  if (typeof ignored === "function") {
    return ignored(path);
  }
  if (Array.isArray(ignored)) {
    return ignored.some((entry) => matchesIgnored(entry, path));
  }
  if (ignored instanceof RegExp) {
    return ignored.test(path);
  }
  return ignored === path;
}
