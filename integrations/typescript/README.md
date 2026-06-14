# DSQL TypeScript Integration

Experimental TypeScript integration for DSQL build metadata.

## Example

After generation, app code can use a DSQL query as a typed operation:

```ts
import { dsql } from "./generated/dsql/queries";
import { useQuery } from "./generated/dsql/tanstack-query";

const MovieInfo = dsql(`
  query MovieInfoLookup {
    movie_info {
      id
      info
    }
  }
`);

export function MovieInfoView() {
  const query = useQuery(MovieInfo, {
    params: {},
    input: {},
  });

  return <pre>{JSON.stringify(query.data, null, 2)}</pre>;
}
```

With the Vite plugin enabled, the inline query is compiled to a generated
operation import. The client-facing operation handle is typed and SQL-free;
generated TanStack Start server functions keep SQL execution on the server.

This package should stay metadata-first:

1. The Rust CLI/compiler emits checked SQL and JSON metadata under
   `dsql/build/manifest.json`.
2. `dsql generate` runs configured commands that write owned application code
   from the manifest.
3. This package provides convenience types and helpers for tools that interact
   with those build artifacts.

The root package exports metadata types and small runtime-neutral helpers. Node
and Bun filesystem helpers live under `@dsql/typescript/node` so browser-facing
code can import the root package without pulling in `node:fs`.

Renderers are intentionally normal Bun/TypeScript programs so projects can
vendor or replace them. The preferred setup is a project-owned entrypoint that
loads DSQL build artifacts and composes generator helpers from
`@dsql/typescript/node`.

```toml
[generate.typescript]
enabled = true
cmd = ["bun", "dsql/generate.ts"]
```

```ts
import {
  loadBuildArtifacts,
  renderDsql,
} from "@dsql/typescript/node";

const artifacts = loadBuildArtifacts(process.env.DSQL_MANIFEST!);

await renderDsql(artifacts, {
  root: process.cwd(),
  queriesDir: "src/generated/dsql",
});

// Add vendored project/framework renderers here.
// They can read artifacts.operations and write app-specific owned files.
```

The minimal built-in entrypoint is `renderers/types.ts`. It only writes the
operation types/constants, the typed `dsql` helper, and barrel exports. The
`renderers/generate.ts` entrypoint is an example of a vendored-style script
that additionally writes starter TanStack Query and TanStack Start wrapper
files without making those TanStack renderers part of the package API.

Project/framework generators and templates should be local to your app. The
example `renderers/generate.ts` is meant to be copied to `dsql/generate.ts`;
after copying, it imports TanStack generator modules from `./generators/*`, and
those generators read templates from `dsql/templates/*`:

```text
dsql/
  dsql.toml
  generate.ts
  generators/
    tanstack-start.ts
    tanstack-query.ts
  templates/
    tanstack-start.ts
    tanstack-query.ts
src/
tsconfig.json
```

A vendored entrypoint should use the shared artifact loader rather than parsing
the manifest shape itself:

```ts
import { loadBuildArtifacts } from "@dsql/typescript/node";

const artifacts = loadBuildArtifacts(process.env.DSQL_MANIFEST!);
```

Vendored generators can import `ts-morph` through
`@dsql/typescript/renderer`, which keeps generation tooling resolvable through
the DSQL package without requiring the generated browser/runtime modules to
depend on it.

## Generated Output

The base generator writes one public module per DSQL definition:

```text
src/generated/dsql/
  MovieInfoLookup.ts
  MovieFields.fragment.ts
  index.ts
```

Each operation module exports its result/input/params types, a typed operation
handle, source-string typing for `dsql(...)`, and, by default, its execution
payload. Browser-facing operation handles are safe to import; SQL placement is
controlled by the layout.

```ts
const MovieInfo = dsql(`query MovieInfoLookup { movie_info { id } }`);
```

For backend-only projects, colocated execution payloads are the simplest layout:

```ts
await renderDsql(artifacts, {
  root,
  queriesDir: "src/generated/dsql/queries",
});
```

For frameworks with client/server import boundaries, pass `executionDir` to
write matching execution modules separately:

```ts
const dsql = await renderDsql(artifacts, {
  root,
  queriesDir: "src/generated/dsql/queries",
  executionDir: "src/generated/dsql/queries.server",
});
```

Output:

```text
queries/
  MovieInfoLookup.ts
  MovieFields.fragment.ts
  index.ts
queries.server/
  MovieInfoLookup.ts
  index.ts
```

DSQL does not enforce framework-specific import protection. Choose paths that
your framework protects, such as a `*.server.*` file pattern or a server-only
directory configured in your application.

`renderDsql` returns metadata for adapter renderers and Vite transforms:

```ts
{
  scope: { name: "frontend", imports: ["shared"] },
  modules: { queries: "./src/generated/dsql/queries/index" },
  definitions: {
    MovieInfoLookup: {
      operationModule: "./src/generated/dsql/queries/MovieInfoLookup",
      executionModule: "./src/generated/dsql/queries.server/MovieInfoLookup",
    },
  },
  files: [],
}
```

Generation writes through a DSQL-owned manifest so unchanged files keep their
mtimes and stale files are removed only when a previous DSQL manifest recorded
ownership.

## Resolution Maps

Projects without explicit maps use one implicit `default` scope. Projects can
define named maps in `dsql/dsql.toml` to keep independent generated surfaces
while sharing imported definitions:

```toml
[resolution.shared]
documents = ["queries/shared/**/*.dsql"]

[resolution.frontend]
documents = ["src/**/*.tsx", "queries/frontend/**/*.dsql"]
imports = ["shared"]

[resolution.api]
documents = ["queries/api/**/*.dsql"]
imports = ["shared"]
```

Each loaded DSQL or embedded document belongs to exactly one scope. Local and
imported duplicate definitions are diagnostics; DSQL source still uses plain
fragment/query names with no namespace syntax.

## Vite Plugin

Export a user-owned generator and pass it to the plugin. Vite keeps a `dsql
daemon` process alive, asks it to compile project metadata, then calls the
generator in-process. The generator return value is the source of truth for
generated query module paths.

```ts
// dsql/generate.ts
import {
  defineDsqlGenerator,
  renderDsql,
} from "@dsql/typescript/node";

export default defineDsqlGenerator(async ({ artifacts, root }) => {
  return renderDsql(artifacts, {
    root,
    queriesDir: "src/generated/dsql",
  });
});
```

```ts
// vite.config.ts
import generateDsql from "./dsql/generate";
import { dsql } from "@dsql/typescript/vite";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [dsql(generateDsql)],
});
```

The transform rewrites named DSQL tags to generated operation imports:

```ts
const MovieInfo = dsql(`query MovieInfoLookup { movie_info { id } }`);
```

becomes:

```ts
import { MovieInfoLookupOperation as MovieInfo } from "/src/generated/dsql/queries";
```

Fragment-only DSQL documents are preserved instead of rewritten, because they
do not represent executable operations:

```ts
const MovieCompany = dsql(`
fragment MovieCompany on movie_companies {
  note
}
`);
```

For multi-scope projects, Vite needs compiler-provided source-file-to-scope
metadata plus the returned render metadata to choose the correct generated query
barrel.

## TanStack Start And Query

The vendored TanStack example adds React Query and TanStack Start helpers:

```text
tanstack-query.ts
tanstack-start.ts
tanstack-start.server.ts
```

The intended app flow is:

- use `dsql(...)` or a generated operation handle in app code
- call `useQuery(operation, { params, input })`
- let the generated Start server function execute the query on the server
- configure database execution in the app's TanStack Start request context

Generated SQL should come from `renderDsql` execution modules. The TanStack
Start example imports those payloads from the returned execution module paths
inside `tanstack-start.server.ts`, which is marked server-only:

```ts
import "@tanstack/react-start/server-only";
```

Database execution is configured by the app through TanStack Start request
context, not by a generated module-level singleton:

```ts
// src/server.ts
import handler, { createServerEntry } from "@tanstack/react-start/server-entry";
import type { DsqlServerContext } from "./generated/dsql/tanstack-start";

type RequestContext = DsqlServerContext;

declare module "@tanstack/react-start" {
  interface Register {
    server: {
      requestContext: RequestContext;
    };
  }
}

export default createServerEntry({
  async fetch(request) {
    return handler.fetch(request, {
      context: {
        dsql: {
          async executeQuery(operation, variables) {
            // Use postgres.js, pg, a pool, or another provider here.
            throw new Error("configure DSQL execution");
          },
        },
      },
    });
  },
});
```

The executor interface is intentionally provider agnostic.

### Validation Hooks

Adapters can use identity validation or call user-authored validators. DSQL does
not depend on a validation library.

```ts
import { z } from "zod";
import type { DsqlVariables } from "@dsql/typescript/runtime";
import { MovieInfoLookupOperation } from "./generated/dsql/queries";

export const MovieInfoVariablesSchema = z.object({
  params: z.object({ id: z.number().int() }),
  input: z.object({}),
}) satisfies z.ZodType<DsqlVariables<typeof MovieInfoLookupOperation>>;
```

```ts
await renderTanStackStart(artifacts, dsql, {
  root,
  outDir: "src/generated/dsql",
  validatorFor(operation) {
    if (operation.name === "MovieInfoLookup") {
      return {
        import: {
          name: "MovieInfoVariablesSchema",
          from: "@/validation/movie-info",
        },
        expression: "MovieInfoVariablesSchema.parse",
      };
    }
    return "identity";
  },
});
```

Directive grammar and validation-schema generation are intentionally deferred.

## Metadata Types

The metadata contract is owned by Rust in the `dsql-metadata` crate. The CLI can
generate the TypeScript integration artifacts from that contract:

```sh
cargo run -p dsql-cli -- generate --target typescript-metadata --out-dir integrations/typescript/src/generated
```

This package keeps generated schema/types under `src/generated`. TypeScript
types are generated directly from the Rust `Facet` metadata via
`facet-typescript`; the JSON Schema is kept as an additional integration
artifact.

```sh
bun run generate
```

The lower-level `metadata-schema` and `metadata-typescript` commands remain
available for debugging or piping metadata into other tools.

The checked-in generated files are bootstrap artifacts while the metadata format
is still moving.
