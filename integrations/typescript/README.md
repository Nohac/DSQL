# DSQL TypeScript Integration

Experimental TypeScript integration for DSQL build metadata.

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

// Add vendored project/framework generators here.
// They can read artifacts.operations and write app-specific owned files.
```

The minimal built-in entrypoint is `renderers/types.ts`. It only writes the
operation types/constants, the typed `dsql` helper, and barrel exports. The
`renderers/generate.ts` entrypoint is an example of a vendored-style script
that additionally writes starter TanStack Query and TanStack Start wrapper
files without exposing those helpers from `@dsql/typescript`.

Template files are split by ownership. Package-owned templates are internal to
`@dsql/typescript`:

```text
templates/bundled/
```

Project/framework templates should be local to your app. The example
`renderers/generate.ts` imports a single copyable template module:

```text
dsql/
  generate.ts
  templates/
    my-templates.ts
```

A vendored entrypoint should use the shared artifact loader rather than parsing
the manifest shape itself:

```ts
import { loadBuildArtifacts } from "@dsql/typescript/node";

const artifacts = loadBuildArtifacts(process.env.DSQL_MANIFEST!);
```

The exact runtime API is still open. This package is intentionally small until
the DSQL build metadata format stabilizes.

## Vite Transform

The Vite plugin is currently a pure transform. It does not run `dsql generate`;
run generation before starting Vite or as part of the project `dev`/`build`
script.

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

The transform rewrites named DSQL tags to generated operation imports:

```ts
const MovieInfo = dsql`
  query MovieInfoLookup {
    movie_info {
      id
    }
  }
`;
```

becomes:

```ts
import { MovieInfoLookupOperation as MovieInfo } from "/src/generated/dsql/queries";
```

The generated `queries` module is a compatibility barrel. Vendored templates
should prefer importing from the specific generated module they need, such as
`operations`, `dsql`, `tanstack-query`, or `tanstack-start`.

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
