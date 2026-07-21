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
query UsersTable @ui.table(total: "users_total") {
  users(limit $$limit offset $$offset) {
    id @ui.column(hidden: true)
    name @ui.column(label: "Name")
    email @ui.column(label: "Email")
  }

  users_total: users | aggregate {
    count
  }
}
```

Metadata directives do not change SQL semantics unless they are explicitly
defined as semantic system directives.

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
  contextRequirements: {} as UsersPageContextRequirements,
  metadata: {}
});
```

`contextRequirements` describes server binding requirements; it is not a value
accepted from the generated client.

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
- required trusted context
- result types and filter-driven nullability
- endpoint or RPC target information
- source maps back to the inline DSQL template

Trusted context such as `$:tenant_id` is bound only by a server-side framework
adapter or request boundary. It is never passed as a public param, emitted as a
browser-client input, or accepted from an untrusted operation payload.

Client cache identity uses a separate opaque `contextScope`. Generated query
helpers require it whenever an operation has trusted-context inputs and include
it in the cache key, but never send it in the execution payload. A scope may be
a non-secret user/session identifier, authorization version, or a derived hash;
it must not contain raw trusted-context values. Applications must change the
scope whenever the trusted context can change the operation result.

```ts
useQuery(VisibleProjects, {
  contextScope: session.authorizationVersion,
  params: { limit: 20 },
});
```

The generated server boundary should preserve the separation explicitly:

```ts
materialize(operation, publicInput, trustedContext)
```

The concrete API is integration-specific, but the contract is not: public input
is validated against the compiled operation surface, trusted context is
validated against the operation's required context metadata, and execution is
refused if required context is missing. The resulting SQL parameter binding may
combine both sources internally while retaining their provenance.

### Generated Input Contracts

Generated public input metadata must preserve each inferred leaf's path,
logical type, collection shape, semantic role, requiredness, nullability, and
typed default. Definition-reference bindings also retain the originating
definition and source span so generators and editor tooling can explain where a
contained or lifted contract came from. Root lifting changes paths but must not
discard defaults or bounded dynamic capability surfaces.

Requiredness and nullability produce distinct host types. For example, a query
header containing:

```dsql
query Movies(
  $$limit = 20
  $$from? = null
) {
  titles(where .production_year >= $$from limit $$limit) {
    id
  }
}
```

conceptually generates:

```ts
type MoviesParams = {
  limit?: number;
  from?: number | null;
};
```

Omission is valid only when metadata provides a default. Explicit `null` is
valid only when the field is nullable. A runtime adapter applies defaults before
validation and execution, so downstream generators do not invent their own
default semantics.

Cache-key builders must use the materialized public input rather than the raw
caller object. Omission and explicitly supplying the declared default therefore
produce the same key. For nullable predicate operands, explicit `null` records
the compiler-defined structural absence of that predicate atom; integrations
must not reinterpret it as SQL null comparison. Cache normalization also maps a
nullable bounded dynamic predicate or order value of `null` to its canonical
empty `{}` or `[]` identity, so semantically identical dynamic inputs share a
key.

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

A split-fetch handoff also carries the compiler-checked parent authorization
chain and trusted-context requirements. Those fields are descriptive inputs to
cache and transport adapters, not client-provided authorization evidence. The
generated child operation re-authorizes the chain server-side and binds current
trusted context at its own request boundary.

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

Filter match locking is resolved by DSQL before artifact publication. Unlocked
`dsql generate` may update `<project-root>/dsql/dsql.lock` from the same
successfully checked snapshot used for generation. `dsql generate --locked`
requires an exact existing lock and never modifies it.

Host integrations select the mode but do not interpret the lock. The Vite
option `locked: true | false | "build"` defaults to `"build"`; that mode uses
unlocked generation during development and locked generation for the Vite
build command. The daemon remains responsible for comparison, semantic diff
diagnostics, and any lock update.

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
Transforms require returned render metadata so each source file can import from
the generated barrel for its owning resolution scope.

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

Root SQL result column aliases must be the validated public DSQL output keys.
Generated runtimes and adapters should not rewrite raw database rows into a
different public shape. Nested relation SQL aliases should remain internal and
continue to be hidden behind JSON object keys selected by the DSQL query shape.

Filters are part of the public result contract. A conditionally filtered scalar
or to-one relation is nullable. A filtered to-many relation remains an array and
may be empty. SQL lowering must apply the same logical readable value in
projection, predicates, ordering, grouping, and aggregate operands; a runtime
adapter must not implement filtering as a post-query output mask.

The initial result contract intentionally does not distinguish database `NULL`
from a value masked by a filter. Metadata may classify the field as conditional
without adding a per-result access mask.

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

Each embedded expression contains exactly one top-level query or fragment.
Semantic analysis publishes that definition's opaque artifact id as the
rewrite target; build adapters map the id to a generated export without
counting definitions or inspecting query/fragment kinds. Host ownership and
extractor identity come from project configuration and the build daemon, never
from an adapter-side extension list.

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
- trusted context values or context-scope identities participating in cache
  identity
- split-fetch cache keys

A cache shared across trusted-context scopes must include a stable scope
discriminator or hashes of the relevant context values. A client cache may rely
on a server-defined session boundary, but its adapter must invalidate or change
scope when authentication context changes. Public variables alone are not a
safe identity for results affected by context-dependent filters.

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
  "filters": [],
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

- public query inputs and top-level params, including requiredness, nullability,
  typed defaults, semantic roles, and definition provenance
- required host context keys
- result field types and nullability
- applicable filters and resolved target provenance
- declaration defaults, trusted enforcement conditions, and query filter
  assignments
- filter-driven conditional fields, classified as context-only or row-dependent
- parameter binding provenance: public input or trusted context
- bounded dynamic predicate and order input surfaces
- split-fetch handoffs, checked parent authorization chains, and cache key inputs
- SQL variants for dynamic operators
- source spans for diagnostics, hovers, and explain output
- provider-specific hints that consumers can safely ignore

Open questions:

- Whether codegen metadata is directive-based, config-based, or both.
- How much metadata can be inferred from the catalog.
- How generated metadata refers to result paths.
- How provider-specific metadata is represented.
- Whether metadata output should be stable JSON, typed Rust structs, or both.
