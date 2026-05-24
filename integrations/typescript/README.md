# DSQL TypeScript Integration

Experimental TypeScript integration for DSQL build metadata.

## Example

After generation, app code can use a DSQL query as a typed operation:

```ts
import { dsql, useQuery } from "./generated/dsql/queries";

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
out_dir = "src/generated/dsql"
cmd = ["bun", "dsql/generate.ts"]
```

```ts
import {
  loadBuildArtifacts,
  renderDsqlHelper,
  renderTypes,
} from "@dsql/typescript/node";

const artifacts = loadBuildArtifacts(process.env.DSQL_MANIFEST!);
const outDir = process.env.DSQL_OUT_DIR!;

await renderTypes(artifacts, { outDir });
await renderDsqlHelper(artifacts, { outDir });

// Add vendored project/framework renderers here.
// They can read artifacts.operations and write app-specific owned files.
```

The minimal built-in entrypoint is `renderers/types.ts`. It only writes the
operation types/constants, the typed `dsql` helper, and barrel exports. The
`renderers/generate.ts` entrypoint is an example of a vendored-style script
that additionally writes starter TanStack Query and TanStack Start wrapper
files without making those TanStack renderers part of the package API.

Renderer files are split by ownership. Package-owned templates are internal to
`@dsql/typescript`:

```text
templates/bundled/
```

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

The exact runtime API is still open. This package is intentionally small until
the DSQL build metadata format stabilizes.

## Generated Output

The base generator writes public TypeScript modules under `src/generated/dsql`:

```text
operations.ts
dsql.ts
index.ts
queries.ts
```

`operations.ts` contains typed operation handles and result/input types.
Browser-facing operation handles do not contain SQL. The typed `dsql` helper
maps inline query strings to those generated operation types:

```ts
const MovieInfo = dsql(`query MovieInfoLookup { movie_info { id } }`);
```

## Vite Plugin

Without a generator, the Vite plugin is a pure transform. This keeps the older
setup working when generation runs separately:

```ts
import { dsql } from "@dsql/typescript/vite";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [
    dsql({
      generatedModule: "/src/generated/dsql/queries",
    }),
  ],
});
```

To let Vite run generation, export a user-owned generator and pass it to the
plugin. Vite keeps a `dsql daemon` process alive, asks it to compile project
metadata, then calls the generator in-process.

```ts
// dsql/generate.ts
import {
  defineDsqlGenerator,
  renderDsqlHelper,
  renderTypes,
} from "@dsql/typescript/node";

export default defineDsqlGenerator(async ({ artifacts, outDir }) => {
  await renderTypes(artifacts, { outDir });
  await renderDsqlHelper(artifacts, { outDir });
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

The generated `queries` module is a compatibility barrel. Vendored templates
should prefer importing from the specific generated module they need, such as
`operations`, `dsql`, `tanstack-query`, or `tanstack-start`.

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

Generated SQL is kept in `tanstack-start.server.ts`, which is marked
server-only:

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
