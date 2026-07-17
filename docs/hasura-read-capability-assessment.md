# Hasura Read Capability Assessment

This document uses a large Hasura-backed application as a structural workload
sample for evaluating dsql. The target is not to port that application's client
or preserve its GraphQL stack. The target is to make dsql capable enough to
supersede the read-side system in a cleaner full-stack architecture.

Dsql remains server-only. Browser code may call generated APIs, but it never
receives SQL or executes dsql directly. Mutations, subscriptions, Apollo
compatibility, and external-service resolution are deliberately excluded from
the priority assessment.

The audited application is kept anonymous. No domain identifiers, business
queries, or source-repository references are reproduced here.

## Summary

A safe full read-side replacement is not close today, but the relational
compiler is much closer than the raw compatibility percentage suggests. The
largest gaps are concentrated in a handful of reusable capabilities rather
than hundreds of unrelated features.

The audit found 193 Hasura queries and 65 reusable table fragments. Thirteen
subscriptions and a small separate GraphQL service were identified but excluded
from the coverage model.

Only about 21 of the 193 queries avoid every currently identified
language-level gap. This does not mean that 172 queries need bespoke features.
Implementing four coherent capability clusters raises estimated coverage
sharply:

| Cumulative capability | Queries covered |
| --- | ---: |
| Current identified surface | 21 / 193 |
| Dynamic and optional variable semantics | 64 / 193 |
| Plus normalized relationship metadata | 80 / 193 |
| Plus core read parity features | 168 / 193 |
| Plus computed fields and table-valued functions | 193 / 193 |

These are feature-shape estimates, not proof that the operations would already
be production-safe. Authorization and the generated server API boundary remain
cross-cutting blockers even after language coverage reaches 193.

## Existing Strengths

The ordinary relational core maps well:

- PostgreSQL tables, views, schemas, constraints, indexes, and foreign keys.
- Deep nested reads; the audited corpus reaches depth 11.
- Composite and reverse foreign-key traversal.
- Relationship predicates.
- Static boolean predicates using comparisons and `and`/`or`.
- Aliases and multi-root operations.
- Offset pagination with `limit`, `offset`, and fixed ordering.
- Table fragments. All 65 relevant fragments target table-shaped types, so
  inline fragments or polymorphic output are not required by this workload.
- Root and nested aggregates. The corpus primarily needs `count`, with limited
  `sum` and `max` use.
- Typed scalar variables, fixed-field dynamic comparison operators, and
  dynamic ascending/descending direction.
- TypeScript result generation and build-time embedded queries.

Features absent from dsql but irrelevant to this read corpus include inline
fragments, read actions, remote schemas, REST endpoints, and conditional
GraphQL directives. `@dsql.include_if` is therefore not a priority for this
assessment.

## Gap-To-Specification Map

### Dynamic boolean filters

- Observed in 78 queries.
- Covered conceptually by [Variables](spec/variables.md), especially bounded
  dynamic filters and ordering.
- The proposed capability model is a strong match.
- Grammar, planning, metadata, list operators, null operators, and code
  generation remain unimplemented.

### Dynamic ordering

- Observed in 80 queries.
- Covered conceptually by [Variables](spec/variables.md).
- The bounded `order on selected` design directly addresses the need.
- Nested allowlists, multiple fields, and explicit null ordering remain open.

### Optional and defaulted variables

- 102 queries declare at least one optional or defaulted variable.
- The corpus contains 303 optional variables and nine defaults.
- [Variables](spec/variables.md) is unfinished in this area.
- Current dsql input metadata makes every variable required and non-null.
- Omitted filters, ordering, limits, offsets, and scalar predicates need
  explicit SQL and type semantics.

### Metadata-defined relationships

- The metadata defines 629 foreign-key-backed relationships and 97 manual
  relationships.
- Manual relationships are selected by 65 queries.
- [Metadata Sources](spec/metadata-sources.md) explicitly considers Hasura
  metadata.
- [Relationship Naming](spec/relationship-naming.md) covers imported names and
  aliases.
- The current catalog only represents relationships as foreign keys. It needs
  a general named relationship model with manual column mappings,
  cardinality, nullability, and source provenance.

### Authorization and role filtering

- The metadata contains 481 read-permission entries across two roles.
- Ninety-nine permission entries add row filters.
- Eighty-three filters depend on request/session values.
- 172 tracked objects expose materially different read permissions between
  roles.
- [Policies And Permissions](spec/policies.md) and
  [Code Generation Metadata](spec/codegen.md) anticipate context values, row
  policies, soft-delete filters, field visibility, and policy-aware types.
- None of this is implemented yet. This is the principal production-safety
  blocker.

### Inferred singular selections

- Primary-key root reads occur in 65 queries.
- [Query Language](spec/query.md#selection-result-cardinality) now defines a
  nullable-object result when mandatory predicates cover a catalog-proven
  unique key or a selection uses literal `limit 1`.
- A selection using only a runtime limit remains array-valued. Redundant limits
  on independently unique selections receive diagnostics instead of changing
  the generated type.
- The remaining work is implementation across semantic planning, SQL result
  assembly, metadata, generated APIs, flattening, and editor services rather
  than a new root-only syntax.

### Aggregates

- Root aggregates occur in 58 queries, with additional nested aggregate use.
- [Aggregates](spec/aggregates.md) covers the required computation and is
  implemented. Its remaining open questions are later extensions rather than
  gaps in the current aggregate contract.
- Hasura and dsql use different wrapper shapes. The replacement stack should
  use dsql's generated API shape rather than preserve GraphQL wrappers.

### Predicate operators

- Twenty-six queries contain immediately unsupported static predicate forms.
- Dynamic filters construct these operators much more broadly in host code.
- [Query Language](spec/query.md),
  [Scoped Predicates](spec/scoped-predicates.md), and
  [Variables](spec/variables.md) cover parts of the problem.
- The practical read set needs `ilike`, `in`, `not in`, `is null`, and
  likely `not`.
- Null comparisons need dedicated lowering; ordinary SQL `= NULL` is not
  equivalent to `IS NULL`.

### Explicit null ordering

- Explicit null placement occurs in ten queries and is also constructed by
  host code.
- It is an open question in [Variables](spec/variables.md).
- It has no accepted syntax or implementation.

### Distinct-on selection

- `distinct on` occurs in eight queries.
- No current specification covers it.
- A design must preserve PostgreSQL's requirement that the initial ordering
  expressions agree with the distinct expressions.

### Computed fields

- Three computed fields are declared and selected by 15 queries.
- [Computed Expressions](spec/computed-expressions.md),
  [Functions](spec/functions.md), and
  [Programmatic Resolvers](spec/programmatic-resolvers.md) cover adjacent
  designs.
- Imported SQL computed fields need provider metadata, row-argument binding,
  selectable-field semantics, result typing, and policy integration.

### Table-valued functions

- Seventeen functions are tracked, and function roots are used by 11 queries.
- [Functions](spec/functions.md) currently focuses on scalar expression
  functions.
- Set-returning functions used as typed root collections need a separate
  provider and planning design.

### Provider scalar types

- Twelve queries select values whose precise scalar type is not represented by
  the current logical type model.
- Thirty queries select closed enum fields.
- The observed gaps include scalar arrays, dates, timestamps without time zone,
  and metadata-defined enums.
- [Catalog Metadata](spec/catalog-metadata.md) covers provider type mapping at
  a coarse level, but arbitrary provider scalars, arrays, and enum values remain
  underspecified.

### Generated server APIs

- [Code Generation Metadata](spec/codegen.md) already separates public query
  handles from server-only execution payloads.
- The current TanStack Start integration demonstrates the intended security
  boundary: generated browser calls cross into server functions, and only the
  server materializes and executes SQL.
- The remaining work is not Apollo compatibility. It is a stable generated API
  contract for validated public input, trusted request context, policy binding,
  operation lookup, execution errors, and cache identity.

## Security And Execution Boundary

The audited database migrations contain no PostgreSQL row-level security
policies. Authorization currently lives in Hasura metadata.

Dsql cannot safely execute equivalent SQL under a broadly privileged database
connection until one of these strategies exists:

1. Translate existing permission metadata into generic dsql policies as
   bootstrap data.
2. Introduce equivalent PostgreSQL row-level security.
3. Apply equivalent constraints in a trusted application layer.

The policy RFC proposes stable result shapes with masked fields, while the
reference system can expose role-specific schemas in which fields are absent.
Dsql does not need to preserve that behavior, but the generated API's chosen
field-visibility and nullability semantics must become normative before policy
implementation.

Policies must apply consistently to relationship traversal, aggregates,
computed fields, and table-valued functions. Protecting ordinary rows while
leaving an aggregate or function path unfiltered would create an authorization
bypass.

Generated browser calls should send an operation identifier and validated
public input to the server. The server must own the generated SQL payload, bind
trusted authorization context, execute the operation, and return the typed
result. SQL must not be shipped to or accepted from the browser.

## Generated API Requirements

The replacement stack should consume generated dsql APIs rather than expose a
general query endpoint. The generated contract should provide:

- browser-safe operation handles with no SQL payload;
- server-only operation lookup and materialization;
- generated validation for public params and bounded dynamic inputs;
- trusted request-context binding that callers cannot spoof;
- policy-aware execution and result typing;
- stable cache keys derived from public input plus relevant trusted context;
- a consistent server error contract;
- framework adapters built on the same generated metadata rather than logic
  embedded in the language core.

## Semantic Edge Cases

Implementation and capability tests should cover:

- Omitted filter, order, limit, and offset variables versus explicit `null`.
- Defaulted variables and conditional clause omission.
- `IN []`, `NOT IN []`, and `NOT IN` containing nulls.
- `IS NULL` and `IS NOT NULL`.
- Case-insensitive matching and database collation behavior.
- PostgreSQL `DISTINCT ON` ordering requirements.
- Explicit null ordering.
- Primary-key misses returning `null`, not `[]`.
- Manual relationships across views, composite mappings, ambiguous names, and
  declared cardinality.
- Permission filters composed with explicit filters and aggregates.
- Field permissions and role-dependent generated types.
- Array scalars, dates, enums, JSON, exact numerics, and large integers.
- Function volatility, session arguments, and set-returning cardinality.
- Deep and wide operations; the largest audited operation expands to more than
  200 selected fields.
- Changes to trusted role/session context and their effect on cache identity.
- Generated API error behavior and transaction boundaries for multi-root
  operations.

## Feature Priorities

### Priority 0: trusted execution and catalog foundations

These capabilities determine whether generated APIs are safe and whether the
compiler can describe the real relational model. They should be designed before
broadening query syntax.

#### 1. Policies, trusted context, and generated API enforcement

Implement the policy/context path across parsing or provider metadata,
planning, SQL, generated metadata, server execution, and result types.

Required behavior:

- host-owned `$:<name>` context that public callers cannot provide;
- implicit row filters and default filters on roots and relationships;
- field and relation visibility;
- identical policy application to ordinary rows, aggregates, computed fields,
  and function-backed sources;
- policy requirements in generated operation metadata;
- cache identity that includes every trusted context value capable of changing
  a result;
- inspectable debug output showing the policies applied to an operation.

Specifications to expand:

- [Policies And Permissions](spec/policies.md): select one field-visibility
  model, define policy composition and overrides, specify aggregate/function
  enforcement, and define errors and debug output.
- [Variables](spec/variables.md): finalize `$:<name>` inference and binding.
- [Code Generation Metadata](spec/codegen.md): define the trusted-context and
  policy contract between generated browser calls and server execution.

#### 2. General relationship metadata and catalog overlays

Replace the assumption that every selectable edge is a PostgreSQL foreign key
with a normalized relationship model. Foreign keys remain one provider of that
model.

Required behavior:

- stable relationship names owned by metadata;
- object and collection cardinality;
- ordered composite column mappings;
- relationships involving views;
- explicit nullability and uniqueness evidence;
- relationship use in selections, predicates, ordering, policies, and lints;
- deterministic merging and conflict diagnostics across introspection and
  project metadata.

Specifications to expand:

- [Catalog Metadata](spec/catalog-metadata.md): add first-class relationship
  metadata rather than storing only foreign keys.
- [Metadata Sources](spec/metadata-sources.md): define the normalized provider
  interface, overlay precedence, provenance, and conflict rules.
- [Relationship Naming](spec/relationship-naming.md): settle per-source-table
  scoping, alias conflicts, default output keys, and raw-edge fallback.

An importer for existing Hasura metadata can be useful bootstrap tooling, but
the normalized catalog contract must not make Hasura the permanent semantic
model.

#### 3. Server-only generated API contract

Generalize the boundary already demonstrated by the TanStack Start renderer.
Framework integrations may differ, but all must preserve the same security
contract.

Required behavior:

- browser-safe operation handles without SQL;
- server-only operation payload lookup;
- generated validation before execution;
- trusted context supplied by the request/server boundary;
- an executor interface for database transactions and connection ownership;
- stable result and error contracts;
- no endpoint that accepts arbitrary SQL or arbitrary query structure.

Specification to expand:

- [Code Generation Metadata](spec/codegen.md): make the public-handle,
  server-payload, validator, context, executor, error, and cache-key contracts
  normative rather than framework examples.

### Priority 1: high-coverage read features

These features account for most of the difference between the current 21-query
surface and the estimated 168-query core read surface.

#### 4. Bounded dynamic filters and ordering

Implement the bounded capability model already proposed in
[Variables](spec/variables.md).

Required behavior:

- explicit field/operator allowlists and reusable presets;
- shallow-by-default relationship exposure;
- typed `and`, `or`, and `not` composition;
- list-valued operators;
- ordered multi-column sort inputs;
- explicit null ordering;
- reuse of one bounded input across detail and aggregate/page-metadata roots;
- generated validators that reject fields and operators outside the compiled
  capability surface.

The specification needs final grammar, metadata shape, relation-depth rules,
empty-input semantics, and SQL lowering. It should use the audited workload as
an acceptance model without reproducing its domain schema.

#### 5. Optional inputs, defaults, and conditional clause participation

Optionality is separate from accepting `null`. The compiler must know whether
an omitted input removes a predicate or clause, supplies a default, or binds SQL
`NULL`.

Required behavior:

- optional public params;
- compile-time defaults represented in generated metadata;
- conditional filter, order, limit, and offset participation;
- stable SQL variants without runtime SQL interpolation;
- correct parameter and cache-key behavior for omitted values;
- compatibility with fragment-bound and reused top-level inputs.

Specification to expand:

- [Variables](spec/variables.md): replace the current open question about
  defaults with exact omission, nullability, variant, and codegen semantics.
- [Directives](spec/directives.md): decide whether conditional participation is
  owned by bounded inputs, `@dsql.include_if`, or distinct mechanisms for
  clauses and result fields.

#### 6. Inferred singular selections

Infer at-most-one result shape for roots and nested relations when mandatory
equality predicates cover a primary key or another catalog-proven unique key,
or when the selection uses literal `limit 1`.

Required behavior:

- nullable object output rather than a one-element array;
- composite unique keys;
- conservative handling of `or`, optional predicates, nullable unique columns,
  and partial or expression indexes;
- literal `limit 1` as an independent proof while runtime limits remain arrays
  unless another proof applies;
- aliases and fragments;
- policy filters that may turn an existing row into an absent result;
- redundant-limit and always-empty diagnostics;
- result metadata and generated API types that distinguish missing from empty.

Specification work:

- [Query Language](spec/query.md#selection-result-cardinality) defines the
  cardinality proofs and limit diagnostics.
- [Catalog Metadata](spec/catalog-metadata.md) supplies relationship and unique
  key evidence. Planning and metadata must retain which proof established the
  result shape.

There is no singular-shape assertion syntax in this increment. When no proof
applies, the generated array type is the feedback. The audited workload does
not require a separate assertion form.

#### 7. Predicate and ordering parity

Extend the static expression surface with the small set repeatedly needed by
real read APIs:

- `ilike`;
- `in` and `not in`;
- `is null` and `is not null`;
- boolean `not`;
- explicit `nulls first` and `nulls last` ordering;
- `distinct on` with PostgreSQL ordering validation.

Specifications to expand:

- [Query Language](spec/query.md) and
  [Scoped Predicates](spec/scoped-predicates.md): define operator typing and
  null/list semantics.
- [Variables](spec/variables.md): define dynamic equivalents and closed SQL
  variants.
- Add a `distinct on` section or specification; it is currently absent.

Important edge cases include empty lists, nulls inside `not in`, collation and
case folding, and the distinction between an omitted predicate and an explicit
null test.

#### 8. Page composition

Basic `limit` and `offset` already work. The next requirement is a single
logical collection definition that can drive rows and page metadata without
duplicating or drifting filters.

Specification to expand:

- [Pagination](spec/pagination.md): define how bounded filters and ordering are
  shared between rows, total counts, and later cursor metadata.

Cursor pagination can remain deferred until the generated API and offset-page
composition are stable.

### Priority 2: provider richness

#### 9. Extensible scalar, enum, and array types

The closed logical type enum should become a provider-capability model capable
of preserving:

- dates and timestamps without time zone;
- scalar arrays;
- closed enum values;
- JSON/JSONB;
- exact numerics and large integers;
- operator and aggregate-function support per type;
- generated wire and validation types.

Specifications to expand:

- [Catalog Metadata](spec/catalog-metadata.md): define arbitrary logical types,
  array element types, enum values, wire representation, and capabilities.
- [Variables](spec/variables.md): define provider-specific input naming and
  validation.
- [Aggregates](spec/aggregates.md): replace hard-coded type allowlists with
  provider capabilities.

#### 10. Provider-backed computed fields

Add selectable scalar fields backed by known database functions or expressions,
without admitting raw SQL into query documents.

Specifications to consolidate or expand:

- [Computed Expressions](spec/computed-expressions.md): distinguish
  query-authored expressions from provider-declared computed fields.
- [Functions](spec/functions.md): define row arguments, return types,
  volatility, nullability, SQL lowering, and policy interaction.
- [Catalog Metadata](spec/catalog-metadata.md): define how computed fields are
  mounted on catalog objects.

External-service resolvers remain out of scope.

#### 11. Table-valued and set-returning functions

Model trusted database functions as typed root or relation sources when they
return a known row shape.

Required behavior:

- typed public and trusted-context arguments;
- declared result object and cardinality;
- filtering, ordering, pagination, and fragments over returned rows where the
  provider supports them;
- volatility and transaction metadata;
- policy enforcement on inputs and returned rows.

Specification to expand:

- [Functions](spec/functions.md): its current scalar-expression focus does not
  cover function-backed collection sources.

## Deferred And Explicit Non-Goals

The following should not compete with the priorities above during this pass:

- mutations;
- subscriptions and live-query transport;
- Apollo compatibility;
- GraphQL result-shape compatibility;
- remote schemas and external-service field resolution;
- client-side dsql or SQL execution;
- unrestricted dynamic query documents;
- cursor pagination before offset-page composition is complete.

Programmatic resolvers may remain a future host-integration feature. Ordinary
backend code in a full-stack application can call external services directly
without making those services part of dsql's relational query language.

## Suggested Delivery Order

1. Settle policies/trusted context and the server-only generated API contract.
2. Introduce normalized relationship metadata and catalog overlays.
3. Implement bounded dynamic filters/order plus optional input semantics.
4. Implement inferred singular selections and the missing
   predicate/order/distinct features.
5. Compose page rows and metadata over one bounded collection input.
6. Generalize provider scalar, enum, and array types.
7. Add provider-backed computed fields and table-valued functions.
8. Re-run the anonymous capability audit after each cluster and add only
   synthetic regression fixtures.

All regression fixtures derived from this assessment should be synthetic. They
should reproduce the required structural shapes without copying domain names,
business queries, or identifiers from the audited application.
