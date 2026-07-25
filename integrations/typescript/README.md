# DSQL TypeScript Integration

Experimental TypeScript integration for DSQL build metadata.

## Example

After generation, app code can use a DSQL query as a typed operation:

```ts
import { dsql } from "./generated/dsql/frontend/queries";
import { useQuery } from "./generated/dsql/frontend/tanstack-query";

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

This package stays metadata-first:

1. The Rust CLI/compiler emits checked SQL and JSON metadata under
   `dsql/build/manifest.json`.
2. `dsql project sync` emits the typed resolution-scope contract at
   `dsql/project.generated.ts`.
3. A project-owned declarative renderer maps compiler-selected terminal targets
   to generators.
4. The Vite binding or one-shot daemon runner publishes the complete desired
   file set and removes stale output.

The root package exports metadata types and small runtime-neutral helpers. Node
and Bun filesystem helpers live under `@dsql/typescript/node` so browser-facing
code can import the root package without pulling in `node:fs`.

Renderers are intentionally normal Bun/TypeScript programs so projects can
vendor or replace them. The preferred setup is a project-owned declarative
entrypoint:

```ts
import {
  targetOutput,
  typescriptDefinitions,
} from "@dsql/typescript/renderer";
import { project } from "./project.generated";
import { tanstackQuery } from "./generators/tanstack-query";
import { tanstackStart } from "./generators/tanstack-start";

const generators = () => [
  project.generator(typescriptDefinitions({ executionDir: "queries.server" })),
  project.generator(tanstackStart()),
  project.generator(tanstackQuery()),
];

export default project.renderer({
  output: targetOutput("src/generated/dsql"),
  targets: {
    api: { generators: generators() },
    frontend: { generators: generators() },
  },
});
```

The generated `project` makes missing, misspelled, and non-terminal targets
TypeScript errors. Runtime generation also compares its compiler-owned contract
hash before invoking generators, so changing `dsql.toml` requires another
`dsql project sync`.

Named scopes must either own documents or import another scope, and a scope may
import each dependency only once. Remove entirely empty scopes, or make an
output-only target explicit with `documents = []` plus `imports = [...]`.

Under a daemon-driven binding (the Vite plugin), rendering is the
binding's job - do **not** also configure a `[generate.typescript] cmd`,
or the flat command channel and the grouped daemon channel render
divergent output. One-shot generation is the explicit
`bun dsql/generate.ts`, which drives its own daemon (see below). The
legacy flat `cmd` channel remains supported for projects without a binding, but
it must use a separate environment-driven entrypoint such as
`renderers/types.ts`; never point it at the daemon-owning `dsql/generate.ts`:

```toml
[generate.typescript]
enabled = true
cmd = ["bun", "dsql/generate-flat.ts"]
# Required under daemon consumers so outputs are watch-excluded:
outputs = ["src/generated/dsql"]
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
  project.generated.ts
  generators/
    tanstack-start.ts
    tanstack-query.ts
  templates/
    tanstack-start.ts
    tanstack-query.ts
src/
tsconfig.json
```

Vendored generators can import `ts-morph` through
`@dsql/typescript/renderer`, which keeps generation tooling resolvable through
the DSQL package without requiring the generated browser/runtime modules to
depend on it.

## Generated Output

The base generator writes one public module per DSQL definition:

```text
src/generated/dsql/
  api/
    queries/
      MovieInfoLookup.ts
      MovieFields.fragment.ts
      index.ts
  frontend/
    queries/
      ...
```

Each operation module exports its result/input/params types, a typed operation
handle, source-string typing for `dsql(...)`, and, by default, its execution
payload. Browser-facing operation handles are safe to import; SQL placement is
controlled by the layout.

```ts
const MovieInfo = dsql(`query MovieInfoLookup { movie_info { id } }`);
```

`renderDsql` is the low-level pure definitions renderer. It returns desired
file contents but never mutates the filesystem:

```ts
const definitions = await renderDsql(artifacts, {
  root,
  queriesDir: "src/generated/dsql/queries",
});
```

For frameworks with client/server import boundaries, pass `executionDir` to
return matching execution modules separately:

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
    "operation/MovieInfoLookup": {
      operationModule: "./src/generated/dsql/queries/MovieInfoLookup",
      executionModule: "./src/generated/dsql/queries.server/MovieInfoLookup",
    },
  },
  files: [
    {
      path: "/project/src/generated/dsql/queries/MovieInfoLookup.ts",
      contents: "…",
    },
  ],
}
```

Project generators send all contents through `files.write(...)`. Only the
binding publishes the validated complete desired set; unchanged files keep
their mtimes, and unlisted files beneath exclusive renderer-owned roots are
removed.

## Resolution Maps

Projects without explicit maps use one implicit `default` scope. Projects can
define named maps in `dsql/dsql.toml` to keep independent generated surfaces
while sharing imported definitions:

```toml
[resolution.shared]
documents = [{ resolver = "dsql", paths = ["queries/shared/**/*.dsql"] }]

[resolution.frontend]
documents = [
  { resolver = "dsql", paths = ["queries/frontend/**/*.dsql"] },
  { resolver = "typescript", paths = ["src/**/*.ts", "src/**/*.tsx"] },
]
imports = ["shared"]

[resolution.api]
documents = [{ resolver = "dsql", paths = ["queries/api/**/*.dsql"] }]
imports = ["shared"]
```

Each loaded DSQL or embedded document belongs to exactly one scope. Local and
imported duplicate definitions are diagnostics; DSQL source still uses plain
fragment/query names with no namespace syntax.

## Vite Plugin

The binding implements the consumer side of `docs/spec/build-daemon.md`:
it keeps a `dsql daemon` alive over line-JSON stdio, watches the project
base (excluding renderer-owned roots, `buildDir`, and declared generator
outputs), forwards file events via `filesChanged`, and rewrites callsite
expressions **exclusively from daemon-provided ranges** - there is no
detection logic in this package. Before splicing, the buffer is verified
against the compile result's SHA-256 `contentHash`; a mismatch triggers
exactly one refresh, then fails deterministically.

Source ownership comes entirely from the daemon. The plugin has no host-file
extension allowlist: it transforms a normalized module only when the compile
result contains a callsite owned by the `typescript` resolver. Other resolver
identities fail with a configuration error instead of being guessed from the
path.

Projects own a generated contract plus renderer descriptor. `ownedRoots` are
known before any invocation (they become `initialize` excludeRoots), and
`render` maps stable artifact ids to generated modules and exports.

```ts
// dsql/generate.ts
import {
  runDsqlRendererFromProject,
} from "@dsql/typescript/node";
import { targetOutput, typescriptDefinitions } from "@dsql/typescript/renderer";
import { project } from "./project.generated";

export const renderer = project.renderer({
  output: targetOutput("src/generated/dsql"),
  targets: {
    api: {
      generators: [project.generator(typescriptDefinitions())],
    },
    frontend: {
      generators: [project.generator(typescriptDefinitions())],
    },
  },
});

export default renderer;

// One-shot generation (`bun dsql/generate.ts`) uses the same daemon
// channel as Vite, so both paths render identical output.
if (import.meta.main) {
  await runDsqlRendererFromProject(renderer);
}
```

```ts
// vite.config.ts
import renderer from "./dsql/generate";
import { dsql } from "@dsql/typescript/vite";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [
    dsql({
      renderer,
      outputMode: "auto",
    }),
  ],
});
```

`outputMode` accepts `"readable"`, `"compact"`, or `"auto"` (the default).
Automatic mode writes formatted SQL template literals during `vite serve` and
single-line JSON string literals during `vite build`. Pin either explicit mode
when generated output is tracked so serving and building do not rewrite it in
different presentations. The one-shot runner defaults to readable output and
accepts the same explicit `"readable"` or `"compact"` selection.

The plugin must run before source-altering transforms (`enforce: "pre"`
is set by the plugin itself): expression ranges apply to the raw file
bytes. The transform replaces only the expression range and hoists one
import per mapped export after the directive prologue:

```ts
const MovieInfo = dsql(`query MovieInfoLookup { movie_info { id } }`);
```

becomes:

```ts
import { MovieInfoLookupOperation as __dsql_MovieInfoLookupOperation } from "/src/generated/dsql/frontend/queries/MovieInfoLookup.ts";
const MovieInfo = __dsql_MovieInfoLookupOperation;
```

Embedded expressions must define exactly one top-level definition. A query
rewrites to its generated operation handle and a fragment rewrites to its
generated typed fragment handle. Empty expressions and expressions containing
multiple definitions are daemon-side errors; shared definitions can live in
plain `.dsql` documents or separate embedded expressions in an imported scope.

The registry augmentation that types `dsql(`...`)` by its exact source
string is keyed only to the daemon-selected target and the extractor-owned
`content_range` recorded in artifact metadata. The renderer slices host bytes
by that range after verifying the compile's content hash, never by scanning.

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
import type { DsqlServerContext } from "./generated/dsql/frontend/tanstack-start";

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
import { MovieInfoLookupOperation } from "./generated/dsql/frontend/queries";

export const MovieInfoVariablesSchema = z.object({
  params: z.object({ id: z.number().int() }),
  input: z.object({}),
}) satisfies z.ZodType<DsqlVariables<typeof MovieInfoLookupOperation>>;
```

```ts
project.generator(tanstackStart({
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
}));
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
