# Code Generation Metadata

Status: RFC.

dsql should be able to emit more than SQL. A checked query can also produce
metadata for application code generation.

The metadata handoff should be language and framework agnostic. TypeScript,
Rust, HTTP endpoints, server functions, CLIs, and other tools should be able to
consume the same checked SQL plus JSON metadata, even if some integrations become
more polished than others.

## Possible Artifacts

- TypeScript result and variable types.
- Runtime validation schemas.
- Query helpers.
- Table column metadata.
- Form metadata.
- Cache key builders.
- Split-fetch helpers.
- Operation metadata JSON.

## Frontend Metadata

```dsql
query UsersTable($limit: int, $offset: int)
  @ui.table(total: "users_total")
{
  users(limit $limit offset $offset) {
    id @ui.column(hidden: true)
    name @ui.column(label: "Name")
    email @ui.column(label: "Email")
  }

  users_total: users.count()
}
```

Metadata directives should not change SQL semantics unless they are explicitly
defined as planning or policy directives.

## TypeScript Inline Queries

Modern TypeScript frameworks should be able to author DSQL inline and let a
build plugin replace it with generated, typed query helpers.

Possible authoring shape:

```ts
const UsersPage = dsql`
  query UsersPage {
    users(limit $$limit) {
      id
      name
      email
    }
  }
`;
```

A Vite, Babel, SWC, or similar plugin could compile the tagged template into a
typed query object.

Possible generated shape:

```ts
const UsersPage = defineDsqlQuery({
  name: "UsersPage",
  sql: "...",
  result: {} as UsersPageResult,
  params: {} as UsersPageParams,
  context: {} as UsersPageContext,
  metadata: {}
});
```

This should be a host integration target, not a requirement of the core
language. The compiler should expose enough metadata that different adapters can
choose their own runtime shape.

## Framework Integration

For TanStack Query or similar data-fetching libraries, generated helpers should
be able to produce stable query options.

Possible use:

```ts
const users = useQuery(UsersPage.queryOptions({ limit: 20 }));
```

Equivalent explicit shape:

```ts
const users = useQuery({
  queryKey: UsersPage.key({ limit: 20 }),
  queryFn: () => UsersPage.fetch({ limit: 20 })
});
```

The generated helper can use DSQL metadata for:

- stable query keys
- public params and dynamic inputs
- required provider context
- result types and policy-driven nullability
- endpoint or RPC target information
- source maps back to the inline DSQL template

Provider context such as `$:tenant_id` should usually be bound by the framework
adapter once per request or app boundary, rather than passed as public params.

### Compiler-Erased Inline Templates

Inline DSQL should ideally be an authoring form that is erased by a host
compiler/plugin. The `dsql` tag is primarily an embedding marker and should not
imply that DSQL is parsed at runtime.

Authoring shape:

```ts
const MyQuery = dsql`
  query MyQuery {
    users(limit $$limit) {
      id
      name
    }
  }
`;

useQuery(MyQuery, {
  params: { limit: 20 }
});
```

A TypeScript host integration can replace the tagged template with a generated
operation object or import from generated code. The runtime wrapper should work
with generated operation metadata, not raw DSQL source.

Possible wrapper shape:

```ts
useQuery(MyQuery, {
  params: { limit: 20 }
});
```

The exact wrapper names are integration choices, but the generated operation
object should carry enough metadata for stable cache keys, SQL execution, input
types, context requirements, and result types.

Fragment runtime APIs are less settled. A fragment may need query-specific
parent context depending on where it is spread, so a globally exported
`MyFragment` handle may be insufficient. A safer generated shape may be
query-scoped:

```ts
useFragment(MyQuery.fragments.UserPosts, user, {
  params: { limit: 10 }
});
```

In this shape the fragment handle is tied to `MyQuery` and can carry the
handoff metadata for the exact result path where it is used. This distinction is
important:

- fragments as source syntax are reusable compile-time composition units
- generated fragment handles may be query/path scoped runtime artifacts

Open questions:

- Whether standalone fragment handles should exist for fragments that do not
  need parent identity or query-specific context.
- How query-scoped fragment handles should be named.
- Whether generated query objects should expose fragments as nested properties,
  a `fragments` map, or framework-specific helpers.
- How much of the fragment context contract should be visible in TypeScript
  types.

## Generation Configuration

Project configuration should describe enabled generation targets. DSQL itself
should compile/check/plan queries and write metadata, while host integrations
render framework-specific files.

Projects can also delegate rendering to an external command:

```toml
[generate.typescript]
enabled = true
cmd = ["bun", "scripts/dsql-generate.ts"]
```

`dsql generate` should still own parsing, checking, planning, SQL generation,
and manifest writing. External commands should consume the build manifest and
write host/framework-specific owned code. The TypeScript generator entrypoint is
the source of truth for generated TypeScript output paths. Rust configuration
does not define a TypeScript `outDir`.

The command should run once per generation target, not once per query. It should
receive compiler-owned paths through environment variables:

- `DSQL_PROJECT_DIR`
- `DSQL_MANIFEST`

The manifest should be written to project-local build state such as
`dsql/build/manifest.json` so users and tools can inspect, debug, and rerun
generators without recompiling the project every time.

## TypeScript Render Contract

The DSQL-owned TypeScript renderer should emit framework-neutral definition
modules. The default public surface is one module per top-level definition plus
a query barrel:

```text
queries/
  MovieLookup.ts
  MovieFields.fragment.ts
  index.ts
```

Each public query module contains:

- result, params, and input types
- a client-safe operation handle
- source-string registry augmentation for the generated `dsql(...)` helper

Execution payloads can be inline for backend-only projects or split into a
separate directory:

```ts
await renderDsql(artifacts, {
  root,
  queriesDir: "src/generated/dsql/queries",
  executionDir: "src/generated/dsql/queries.server",
});
```

When execution is split, public query modules must not contain SQL text or
execution payload data. Framework adapters import public operation handles from
the query barrel and execution payloads only from the protected execution
surface.

Generated query barrels should export the `dsql` helper, public runtime types,
and every per-definition module so source-string typing is visible from one
import.

Host generators may return render metadata for Vite and other transforms:

```ts
const generator = defineDsqlGenerator(async ({ artifacts, root }) => {
  const dsql = await renderDsql(artifacts, {
    root,
    queriesDir: "src/generated/dsql/queries",
    executionDir: "src/generated/dsql/queries.server",
  });

  return dsql;
});
```

Returned metadata should include the query barrel module, generated files,
operation modules, execution modules, and scope name when generation is scoped.
Single-scope projects may still use a static generated module in a host plugin.
Multi-scope transforms require returned render metadata so each source file can
import from the generated barrel for its owning resolution scope.

## Runtime Result Contract

Generated result types describe the public DSQL output shape. Runtime execution
must return values with those same public keys.

Internal SQL aliases are compiler implementation details. They may be hashed or
shortened for PostgreSQL identifier safety, but they must not leak into the
object returned to user code.

For a root selection such as:

```dsql
query FeaturedMovie {
  movie_info_idx(limit 1) {
    id
  }
}
```

the runtime-facing result must be shaped like:

```ts
{
  movie_info_idx: [{ id: 1 }]
}
```

not:

```ts
{
  movie_info_idx_result_1b1fd7c6: [{ id: 1 }]
}
```

The implementation may satisfy this contract by making root SQL result column
aliases equal to validated public output keys, or by carrying explicit metadata
that maps internal root SQL columns back to public DSQL output keys before
returning data to generated clients. Nested relation SQL aliases should remain
internal and continue to be hidden behind JSON object keys selected by the DSQL
query shape.

## Embedded Language Tooling

Inline DSQL in tagged template literals should be treated as virtual DSQL
documents by editor tooling.

```ts
const Query = dsql`
  query Query {
    users {
      id
    }
  }
`;
```

The LSP should eventually support diagnostics, completion, hover, formatting,
and go-to-definition inside the template content. The source map should retain
both the TypeScript file span and the DSQL virtual document span.

## Rust Integration

Rust could consume DSQL through a build step, generated modules, or a procedural
macro. A macro shape might generate a module with params, result types, metadata,
and an optional fetch helper.

```rust
dsql::query! {
    pub mod users_page {
        users(limit $$limit) {
            id
            name
        }
    }
}

let result = users_page::fetch(
    &pool,
    users_page::Params { limit: 20 },
).await?;
```

The exact API is unresolved. The useful property is that a Rust integration can
reuse the same checked SQL, variable metadata, context requirements, and result
shape metadata as other integrations.

Other plausible Rust shapes:

```rust
mod users_page {
    include!(concat!(env!("OUT_DIR"), "/users_page.rs"));
}
```

```rust
let query = dsql::include_query!("queries/users_page.dsql");
```

These are integration choices. They should not require a different core metadata
model.

## Identity Metadata

Generated metadata may need stable identity information:

- object identity fields
- result paths
- relation parent-child keys
- variables participating in cache identity
- policy context values participating in cache identity
- split-fetch cache keys

## Metadata Shape

The exact metadata format is unresolved, but generated artifacts should expose a
stable handoff from DSQL to host integrations.

Possible high-level shape:

```json
{
  "query": "Projects",
  "result": {},
  "input": {},
  "params": {},
  "context": {},
  "policies": [],
  "dynamic_inputs": {},
  "handoffs": [],
  "sql": {},
  "source_map": {}
}
```

The metadata does not need to mirror the internal compiler IR. It should be a
consumer-friendly contract for generated clients, endpoint adapters, validation,
debug tooling, and editor features.

Useful metadata areas:

- public query inputs and top-level params
- required host context keys
- result field types and nullability
- policy-driven field visibility
- dynamic filter and order input surfaces
- split-fetch handoffs and cache key inputs
- SQL variants for dynamic operators
- source spans for diagnostics, hovers, and explain output
- provider-specific hints that consumers can safely ignore

Open questions:

- Whether codegen metadata is directive-based, config-based, or both.
- How much metadata can be inferred from the catalog.
- How generated metadata refers to result paths.
- How provider-specific metadata is represented.
- Whether metadata output should be stable JSON, typed Rust structs, or both.
