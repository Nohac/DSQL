# Rework TypeScript generation around DSQL owned per definition modules

**ID:** 666a070c | **Status:** Done | **Created:** 2026-06-13T14:23:24+02:00

## Summary

Rework TypeScript generation so DSQL owns the canonical generated output shape,
execution materialization primitives, and Vite transform metadata. Framework
generators should consume this DSQL-owned shape instead of owning SQL payload
layout themselves.

## Context

The current TypeScript integration has DSQL-owned type/helper generation, but
TanStack Start generation owns the SQL-bearing operation descriptors and SQL
variant materialization. That makes the canonical DSQL TypeScript runtime shape
unclear and makes future scoped generation harder to keep deterministic.

DSQL should generate operation/fragment modules and execution payload modules in
a framework-neutral way. Optional adapters, such as TanStack Start, TanStack
Query, API route generators, or custom app generators, should use those modules
and DSQL runtime primitives to create their wrappers.

## Proposed Shape

`renderDsql` writes one public file per top-level definition by default:

```text
queries/
  MovieLookup.ts
  MovieFields.fragment.ts
  index.ts
```

Each public query file contains:

- result/input/params types
- a public operation handle safe to import from client code
- source-string registry augmentation for the generated `dsql(...)` helper

Execution payloads are colocated by default for simple backend-only projects.
If `executionDir` is configured, execution payloads are written to a separate
directory with matching filenames and no special suffix:

```ts
await renderDsql(artifacts, {
  root,
  queriesDir: "src/generated/dsql/frontend/queries",
  executionDir: "src/generated/dsql/frontend/queries.server",
});
```

Output:

```text
queries/
  MovieLookup.ts
  MovieFields.fragment.ts
  index.ts

queries.server/
  MovieLookup.ts
  index.ts
```

The split is generic. DSQL should not encode framework terms such as "server" in
its core model. Users choose path names that match their framework's import
protection rules, such as `queries.server` for TanStack Start or a protected
directory for another framework.

The renderer should document framework-specific import protection expectations,
but should not enforce them. For example, TanStack Start's default protection is
based on file patterns such as `*.server.*`; protecting a directory such as
`queries.server/` may require user configuration in that framework.

## Runtime Primitives

Move framework-neutral execution helpers into `@dsql/typescript`, for example:

- `materializeDsqlQuery(payload, variables)`
- `applyDsqlVariants(sql, variants, variables)`
- `collectDsqlParameterValues(parameters, variables)`
- `getDsqlPath(value, path)`
- shared operation, fragment, variable, and execution payload types

Generated execution files should contain query-specific data: SQL text,
parameter paths, variants, and the operation-to-payload binding. They should not
duplicate the generic materialization implementation.

## Source String Typing

Avoid one large central source-string map. Use TypeScript module augmentation so
each generated definition file owns its own `dsql(...)` source override:

```ts
declare module "../runtime" {
  interface DsqlSourceRegistry {
    readonly "query MovieLookup { movie_info { id } }": typeof MovieLookupOperation;
  }
}
```

This is type-only and is erased from emitted JavaScript. The generated barrel
should export the per-definition modules so the augmentations are visible when a
user imports `dsql` from the generated queries module.

The augmentation target must be stable and exact. Every generated definition
module should augment the same runtime module that exports `dsql`, and tests
must verify that importing from the generated query barrel makes the
augmentations visible to the TypeScript checker.

## Generator Contract

The user-owned generator should return render information so the Vite plugin can
transform files deterministically:

```ts
const generator = defineDsqlGenerator(async ({ scope, artifacts, root }) => {
  const dsql = await renderDsql(artifacts, {
    root,
    queriesDir: "src/generated/dsql/frontend/queries",
    executionDir: "src/generated/dsql/frontend/queries.server",
  });

  await renderTanStackStart(artifacts, dsql);
  await renderTanStackQuery(artifacts, dsql);

  return dsql;
});
```

Returned information should include at least:

- query barrel module used by Vite transforms
- execution module per operation
- generated file paths
- scope name when generation is scoped

If execution is inline, the operation module and execution module can be the
same. If execution is split, adapters import execution payloads only inside
their framework's protected boundaries.

Generators return render metadata. Vite uses that metadata as the source of
truth for transformed imports:

- Scoped Vite transforms should require returned render metadata and fail with a
  clear error if it is missing.
- `runDsqlGeneratorFromEnv`, Vite generation, and CLI-triggered generation
  should go through the same TypeScript generator pipeline.
- Rust should not own TypeScript output paths after this change. Rust should
  provide scoped build artifacts and compiler metadata; the TypeScript generator
  owns layout, output paths, render metadata, and file-write planning.
- Existing Rust `generate.typescript.out_dir` behavior should be removed rather
  than preserved. The TypeScript generator entrypoint is the source of truth for
  output layout.

For scoped projects, render metadata alone is not enough. The Vite plugin also
needs a deterministic source-file-to-scope lookup from the compiler/daemon or
project config so it can choose the right query module for each transformed
file.

Generated names need two collision checks: one for filesystem paths and one for
TypeScript export names. Names that differ in DSQL can still collapse to the
same generated file stem or exported symbol after sanitization/PascalCase
conversion, and those must be diagnostics rather than overwrites.

## Done When

- DSQL-owned `renderDsql` emits per-definition public modules and optional
  split execution modules.
- The existing single-output behavior remains available for small/backend-only
  projects.
- Generic SQL materialization and variant application live in
  `@dsql/typescript`, not in TanStack-specific templates.
- The generated `dsql(...)` helper keeps exact source-string type inference via
  per-module TypeScript augmentation.
- Vite generation consumes the returned render result for transform imports
  instead of relying only on static plugin options.
- The plugin can determine the owning resolution scope for a transformed source
  file before choosing an import module.
- TanStack renderers consume DSQL-owned operation and execution modules rather
  than owning SQL payload layout.
- Documentation explains how to choose an execution directory that participates
  in framework import protection.
- Obsolete flat-layout APIs such as `renderTypes`, `renderDsqlHelper`, and the
  generated root `operations.ts`/`dsql.ts`/`queries.ts` surface are removed
  instead of kept as compatibility shims.
- Filesystem and TypeScript export-name collisions are reported deterministically.
- TypeScript output paths are determined by the TypeScript generator, while Rust
  continues to produce build artifacts/cache metadata independent of output
  layout.
- Tests cover inline execution, split execution, source-string typing,
  transformed imports, and no SQL in client-importable modules when execution is
  split.
