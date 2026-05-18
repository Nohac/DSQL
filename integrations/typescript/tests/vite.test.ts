import { expect, test } from "bun:test";
import { transformDsqlTags } from "../src/vite";

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

test("rejects JavaScript interpolation in dsql tags", () => {
  expect(() =>
    transformDsqlTags("const Users = dsql`query Users { users(where .id == ${id}) { id } }`;"),
  ).toThrow("dsql templates do not support JavaScript interpolation");
});
