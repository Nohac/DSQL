# Code Generation Metadata

Status: implemented for language-neutral metadata, declarative project
renderers, and the browser-handle/server-payload TypeScript split. Additional
renderer and validation targets remain in progress.

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
typed default. Every input, dynamic-input, and result field carries a required
wire record and, for provider text casts, the schema-qualified provider type.
Structural result and input fields use the explicit `unsupported` encoding;
missing wire metadata is an invalid artifact, not a cue to infer from the
logical type name.
This is one compiler-owned contract consumed by native execution and host
runtimes; generators must not infer encodings again from logical type names.
Definition-reference bindings also retain the originating definition and source
span so generators and editor tooling can explain where a contained or lifted
contract came from. Root lifting changes paths but must not discard defaults.
Retaining bounded dynamic capability surfaces is part of the eventual fragment
extension; the initial bounded-dynamic slice rejects dynamic inputs owned by
fragments.

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

DSQL owns parsing, checking, planning, SQL, terminal-target classification, and
artifact metadata. Host integrations render framework-specific files from that
checked handoff. They do not reconstruct scopes or compiler semantics.

Projects can also delegate rendering to an external command:

```toml
[generate.typescript]
enabled = true
cmd = ["bun", "scripts/dsql-generate.ts"]
```

`dsql generate` should still own parsing, checking, planning, SQL generation,
and manifest writing. External commands should consume the build manifest and
write host/framework-specific owned code. The TypeScript project entrypoint is
the source of truth for generated TypeScript output paths; Rust configuration
does not define a TypeScript `outDir`.

The environment-driven command channel receives the flat manifest through
compiler-owned environment variables:

- `DSQL_PROJECT_DIR`
- `DSQL_MANIFEST`

The manifest is written to project-local build state such as
`dsql/build/manifest.json` so users and tools can inspect, debug, and rerun
flat generators without recompiling the project every time. Terminal-target
rendering uses the grouped daemon handoff until the manifest format grows the
same group contract; the two channels must not invent different target rules.

The Vite binding and npm initializer do not configure an additional
`[generate.typescript] cmd`. A daemon consumer invokes the project renderer
itself, and running both channels would create competing generated trees.

Filter match locking is resolved by DSQL before artifact publication. Unlocked
`dsql generate` may update `<project-root>/dsql/dsql.lock` from the same
successfully checked snapshot used for generation. `dsql generate --locked`
requires an exact existing lock and never modifies it.

Host integrations select the mode but do not interpret the lock. The Vite
option `locked: true | false | "build"` defaults to `"build"`; that mode uses
unlocked generation during development and locked generation for the Vite
build command. The daemon remains responsible for comparison, semantic diff
diagnostics, and any lock update.

## Generated TypeScript Project Descriptor

DSQL emits a reproducible source contract at:

```text
dsql/project.generated.ts
```

Conceptually:

```ts
import { defineDsqlProject } from "@dsql/typescript/renderer";

export const project = defineDsqlProject({
  contractHash: {
    algorithm: "sha256",
    value: "…lowercase hex…",
  },
  scopes: {
    shared: { imports: [] },
    api: { imports: ["shared"] },
    frontend: { imports: ["shared"] },
  },
  targets: ["api", "frontend"],
  directives: {},
});
```

Literal scope names, imports, terminal targets, and directive definitions remain
available to TypeScript's type system. The descriptor also gives runtime
renderer code the expected target set, so a stale descriptor cannot silently
route a newly changed project.

`contractHash` is the compiler-owned fingerprint of the canonical scope graph,
terminal-target classification, and generator-visible normalized directive
registry. Its canonical representation is `{ algorithm: "sha256", value:
"<lowercase hex>" }` in both the descriptor and daemon protocol; comparison is
structural over that representation. Before invoking any generator, the
renderer requires both fingerprints and the explicit target graph to agree;
otherwise it reports an actionable descriptor regeneration error and publishes
nothing. This prevents a changed directive argument schema from being consumed
under stale TypeScript types even when the scope names did not change.

The fingerprint is semantic: source-file ordering, JSON/YAML formatting, and
unused raw-schema annotations do not change it, while every change that affects
the generated target or directive TypeScript contract must change it.

This descriptor is generated source, not disposable build state. It contains no
credentials, artifact list, SQL, generation result, or mutable ownership
manifest. Projects may commit it so `generate.ts` type-checks in a fresh
checkout, and supported tooling can recreate it from `dsql.toml` plus registered
directive definitions. Publication manifests and transient state remain under
`dsql/build/`.

An explicit imported project descriptor is preferred to ambient module
augmentation. Multiple DSQL projects may be visible to one TypeScript program;
their scope and directive contracts must not merge invisibly.

## Declarative Project Wiring

`dsql/generate.ts` maps terminal targets to project-owned generators:

```ts
import {
  targetOutput,
  typescriptDefinitions,
} from "@dsql/typescript/renderer";

import { project } from "./project.generated";
import { tanstackQuery } from "./generators/tanstack-query";
import { tanstackStart } from "./generators/tanstack-start";

export default project.renderer({
  output: targetOutput("src/generated/dsql"),
  targets: {
    api: {
      generators: [typescriptDefinitions(), tanstackStart],
    },
    frontend: {
      generators: [typescriptDefinitions(), tanstackQuery],
    },
  },
});
```

The target keys are exactly the terminal targets described by
[Resolution Scopes](resolution-scopes.md). Unknown keys, missing decisions, and
descriptor/compiler disagreement are errors. A target that intentionally emits
nothing uses an explicit typed `project.ignore()` decision; ignoring a target
that owns embedded callsites is invalid because those callsites require render
map entries.

Non-terminal scopes never appear as wiring targets. Their standalone artifacts
already occur in each reachable target's effective artifact view.

Framework-neutral definitions are explicit generator wiring, not implicit
renderer output. Every operation or fragment selected for definition-module
rendering is owned by exactly one definitions generator, which emits its
definition and barrel modules plus the mappings required by callsite rewriting.
A target that owns embedded callsites must therefore wire a definitions
generator. A callsite-free target may omit it when its generators intentionally
produce only other output, such as metadata.

`generate.ts` contains no:

- artifact-group fallback or iteration;
- scope import resolution;
- daemon startup, shutdown, or retry handling;
- DSQL diagnostic or error reformatting;
- output-owner bookkeeping;
- stale-file cleanup; or
- direct publication logic.

Those are stable renderer-library responsibilities. Framework-specific policy
and application conventions remain in project-owned generators and templates,
as specified by [TypeScript Distribution And Project
Wiring](typescript-distribution.md).

## Generator Contract

A generator receives one typed terminal target, its effective checked artifacts,
and a path-safe desired-file collector. It returns desired files and optional
module/export mappings; it does not write the filesystem directly.

The optional `targets` declaration restricts where a generator may be wired.
Omitting it allows every terminal target. Assigning a generator to a target
outside its declaration is a project-contract error before daemon startup.

Conceptually:

```ts
export const tanstackQuery = project.generator({
  name: "tanstack-query",
  targets: ["frontend"],
  render({ target, operations, files }) {
    for (const operation of operations) {
      files.write(
        `tanstack-query/${operation.name}.ts`,
        renderOperation(operation),
      );
    }
  },
});
```

The renderer library owns:

- terminal-target dispatch and effective-artifact projection;
- contextual errors carrying generator and target identity;
- normalized project-relative path validation;
- composition of generated modules, exports, and desired files;
- complete render-map validation;
- atomic state swaps in a binding; and
- owned-root reconciliation and stale cleanup after successful rendering.

Custom generator errors retain their original cause. Project code does not catch
and translate DSQL lifecycle errors.

### Conditional generation

Conditional generation is generator policy, not daemon or `generate.ts` policy.
A generator may select operations by name, arbitrary metadata predicate, or
checked directive:

```ts
for (const operation of operations.named("MovieSearch")) {
  // A project-specific special case.
}

for (const operation of operations.withDirective("tanstack.query")) {
  // Reusable source-declared generation intent.
}

for (const operation of operations.where((operation) => isPublic(operation))) {
  // Any application-owned metadata policy.
}
```

Name and predicate helpers are conveniences; a generator may inspect the full
stable operation metadata. Directive selectors use the generated checked
directive contract described by [Directives](directives.md). Generators never
parse DSQL source, walk CST data, resolve directive names, or validate directive
arguments themselves.

## Output Composition And Collisions

`targetOutput("src/generated/dsql")` assigns a deterministic target-qualified
layout:

```text
src/generated/dsql/api/
src/generated/dsql/frontend/
```

This is the safe default. Imported shared artifacts may appear in both trees,
but every target remains self-contained and no generated module depends on
another target's output.

Custom layouts are allowed, but the renderer normalizes and validates the
complete desired file set before touching disk. Every layout declares stable
owned roots independent of the current target set; custom target paths are
relative to those roots. This lets the binding exclude outputs before daemon
initialization and clean removed targets after the scope graph changes.

The renderer rejects:

- paths escaping an owned root;
- owned roots that overlap ambiguously instead of normalizing to one owner;
- two generators or targets producing the same normalized file path;
- two incompatible module/export mappings for one artifact; and
- authored files placed beneath a declared owned root.

Different generators may safely produce distinct files beneath one common owned
root. Collision detection operates on normalized target files, not merely on
directory strings.

A renderer descriptor declares its owned roots before daemon initialization,
and its render map lists the complete desired files beneath them. Once every
generator and render-map check succeeds, the binding removes unlisted files and
empty stale directories before exposing the new map. Failure publishes nothing
and preserves the previous successful render state. Generated source trees are
reconstructible; disposable compiler metadata remains under `dsql/build`.

## TypeScript Definition Output

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
typescriptDefinitions({
  queriesDir: "queries",
  executionDir: "queries.server",
});
```

When execution is split, public query modules must not contain SQL text or
execution payload data. Framework adapters import public operation handles from
the query barrel and execution payloads only from the protected execution
surface.

Host integrations select a generated-source presentation mode and pass it
through the renderer context:

- `readable` emits formatted SQL as a TypeScript template literal. The
  TypeScript printer owns literal escaping, and evaluating the generated module
  must recover the metadata text byte-for-byte.
- `compact` emits the compiler's semantically identical single-line SQL as a
  JSON string literal.

The compiler derives both forms from one SQL statement and publishes them as
`sql.text` and `sql.compact_text`; presentation never changes operation
identity, parameter ordering, variants, manifests, or owned output paths.
Vite's `outputMode: "auto"` default selects `readable` while serving and
`compact` while building. An explicit mode applies to both commands. Projects
that commit generated output should pin one mode to avoid serve/build-only
file churn.

Generated query barrels should export the `dsql` helper, public runtime types,
and every per-definition module so source-string typing is visible from one
import.

The definitions generator returns the query-barrel module, generated files,
operation modules, execution modules, and terminal target name as structured
module/export mappings. The renderer library combines those mappings into the
complete render map used by transforms, so each source file imports from the
generated barrel for its owning terminal target.

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
  "input": [],
  "params": [],
  "context": [],
  "filters": [],
  "dynamic_inputs": [],
  "handoffs": [],
  "sql": {},
  "source_map": {}
}
```

The metadata does not need to mirror the internal compiler IR. It should be a
consumer-friendly contract for generated clients, endpoint adapters, validation,
debug tooling, and editor features.

Before the first public release, manifest version 5 is a compiler-package
contract rather than a backwards-compatible interchange promise: maintained
compiler, daemon, and renderer components move together, and a required metadata
addition causes a version bump and artifact regeneration. Consumers reject
other versions explicitly; they never synthesize missing execution data.

Useful metadata areas:

- public query inputs and top-level params, including requiredness, nullability,
  typed defaults, semantic roles, and definition provenance
- required host context keys
- result field types and nullability
- explicit result value shape (`scalar`, provider `database_array`, or relation
  `object`), with database arrays carrying the scalar element wire contract
- public wire encodings and schema-qualified provider cast targets for result,
  input, context, and bounded dynamic fields
- applicable filters and resolved target provenance
- declaration defaults, trusted enforcement conditions, and query filter
  assignments
- filter-driven conditional fields, classified as context-only or row-dependent
- parameter binding provenance: public input or trusted context
- bounded dynamic predicate and order input surfaces in server execution
  artifacts; browser operation objects retain only their ordinary public
  input/default contracts
- split-fetch handoffs, checked parent authorization chains, and cache key inputs
- SQL variants for dynamic operators
- checked directive occurrences at their semantic attachment paths, using
  canonical names and validated arguments
- source spans for diagnostics, hovers, and explain output
- provider-specific hints that consumers can safely ignore

Open questions:

- Which generation concerns should be source-declared through checked directives
  versus project-owned renderer configuration.
- How much metadata can be inferred from the catalog.
- How generated metadata refers to result paths.
- How provider-specific metadata is represented.
- Whether metadata output should be stable JSON, typed Rust structs, or both.
