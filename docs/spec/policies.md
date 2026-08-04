# Filters And Access Rules

Status: implemented for the initial scope. Result access masks and the open
extensions at the end remain deferred.

Filters provide reusable, compiler-enforced data constraints and
runtime-dependent readable views for generated operations. They protect
generated endpoints from caller-controlled input and prevent application
queries from accidentally omitting declared constraints.

DSQL operations are fixed at compile time. The query author chooses the roots,
fields, relations, aggregates, and bounded dynamic inputs exposed by an
operation. Filters do not sandbox a trusted author who can change project
source and regenerate. Changing a filter is a code-review and deployment
decision, like changing authentication middleware.

Public callers control only explicitly generated inputs. Trusted request
context remains separate because callers must not be able to forge values such
as tenant identity or administrative status.

## Definitions And Visibility

Filters and reusable conditions are ordinary DSQL definitions. They live in
standalone `.dsql` documents, not embedded regions: neither definition produces
a host-language runtime value for an embedding to reference.

Definitions follow normal resolution-scope visibility. A filter applies in its
declaring scope and scopes that import it. Unrelated scopes do not implicitly
share filters. Names are unique within an effective scope and provide stable
identities for query clauses, metadata, diagnostics, and the match lock.

## One Filter Model

A filter may affect a table, scalar field, or relation:

- A table rule removes rows from the readable collection.
- A scalar-field rule makes the field behave as `NULL` when its condition is
  false.
- A relation-field rule makes the relation behave as absent when its condition
  is false.

The same definition can be manually selected, active by default, conditionally
enforced, or always enforced. Application behavior is declared inside the
filter rather than by different definition keywords.

A filter contains at least one row or field rule. It has at most one row
`where`; authors combine row predicates with the ordinary boolean operators.
Field rules may repeat for different field lists.

## Targets

A filter may target one concrete catalog object:

```dsql
filter RecordingAccess on public::recordings {
  apply where true
  where .project.project_users.user_id == $:user_id
}
```

It may instead target every catalog object compatible with a structural shape:

```dsql
filter TenantScope on {
  .tenant_id: uuid
} {
  apply where true
  where .tenant_id == $:tenant_id
}
```

Shape matching uses exposed catalog names and logical types. A matched field
must have a type compatible with the declared shape type. Relationship
requirements may be added later, but must participate in the same lock
workflow.

Initial structural target shapes declare scalar catalog fields only. The
collection suffix defined for trusted-context declarations in
[`variables.md`](variables.md) is not valid in filter or condition target
shapes.

For the initial structural matcher, every field referenced by a row rule or
named by a field rule must be declared in the target shape. Relationship paths
in rules require a concrete target until structural relationship requirements
are specified. This keeps a shape from first matching a broad set of tables and
then failing opportunistically on undeclared rule dependencies.

A filter that matches no target is a blocking diagnostic. Changes to its
resolved target set block locked generation until accepted in `dsql.lock`.

PostgreSQL views receive no special underlying-table inference. A view matches
when its exposed catalog interface satisfies the shape. Missing exposed fields
or relationship metadata do not match and cannot be recovered by inspecting
the view definition.

## Reusable Conditions

Conditions remove repeated trusted predicates from filter rules:

```dsql
condition AdminOrSelf on {
  .id: uuid
} {
  where $:is_admin or .id == $:user_id
}

condition Admin {
  where $:is_admin
}
```

`on` accepts the same concrete or structural target contract as a filter. It is
omitted for a context-only condition. A targetless condition that references a
field is a diagnostic directing the author to add `on`.

The initial condition contract is intentionally small:

- Conditions may use literals, boolean operators, trusted `$:` context, and
  fields declared by their target.
- Public and inferred `$` or `%` variables are not allowed.
- Conditions are usable only by filter definitions.
- A condition does not take parameters or reference another condition.
- A filter target using a condition must satisfy the condition's target.
- Conditions used by `apply where` must be context-only.

A condition target is a type-checking requirement, not an ambient application
target. The lock records the filters that actually apply and the conditions
they use; it does not independently match a condition against every compatible
catalog object.

A referenced condition is a boolean predicate atom. Filter rules may combine it
with ordinary boolean operators and additional predicates. The condition is
resolved and type-checked once rather than copied as source text at each use.

## Application

`apply` defines the inherited desired state and any irreducible enforcement
condition:

```dsql
filter Published on {
  .published_at: timestamptz
} {
  where .published_at is not null
}
```

With no `apply`, the filter is inactive until selected by a query.

```dsql
filter SoftDelete on {
  .deleted_at: timestamptz
} {
  apply
  where .deleted_at is null
}
```

Bare `apply` makes the filter active by default but freely controllable by the
query.

```dsql
condition PreventReadDeleted {
  where not $:can_read_deleted
}

filter SoftDelete on {
  .deleted_at: timestamptz
} {
  apply where PreventReadDeleted
  where .deleted_at is null
}
```

`apply where <condition>` keeps the filter active whenever its context-only
condition is true, regardless of query preference. It is still active by
default when the condition is false, but a query may then turn it off.

`apply where true` is the always-enforced form. `apply where false` is
equivalent to bare `apply` and should be simplified by the formatter.

For one filter at one source:

```text
desired = query assignment, inherited operation assignment, or declaration default
active  = enforcement condition or desired
```

The declaration default is `true` when `apply` is present and `false`
otherwise. The enforcement condition is the expression following `apply
where`, or `false` for bare or absent `apply`.

Application conditions are evaluated once from trusted context for an
operation execution. They cannot depend on a row, public input, or inferred
variable. Public input controls query preference separately.

The compiler may specialize SQL variants or bind the activation expression as
a boolean parameter, but the semantics are fixed. For activation value `A`, a
row rule `P`, and a field rule `C`:

```text
row remains readable  = not A or P
field remains readable = not A or C
```

Multiple applicable filters compose their resulting guards with `and`.

## Query Filter Assignments

One collection clause assigns the desired state of a named filter:

```dsql
posts(filter Published) {
  id
}
```

Without `when`, the assigned state is `true`. A row-independent boolean
condition assigns the state dynamically:

```dsql
posts(filter Published when %publishedOnly) {
  id
}
```

A default-active filter is turned off by assigning `false`:

```dsql
users(filter SoftDelete when not %includeDeleted) {
  id
  deleted_at
}
```

A static opt-out is explicit:

```dsql
users(filter SoftDelete when false) {
  id
  deleted_at
}
```

With the conditionally enforced `SoftDelete` above, effective application is:

```text
PreventReadDeleted or not %includeDeleted
```

An unauthorized request therefore keeps the filter active. An authorized
request still receives the default filtered view until it explicitly requests
deleted rows.

The named filter must be visible and match the source. Unknown names,
nonmatching filters, and duplicate assignments for the same filter at one scope
are diagnostics. An assignment to a statically always-enforced filter is
redundant when it is statically `true` and a diagnostic when it can be `false`;
it never silently suggests that the filter can be disabled.

## Operation-Wide Assignments

Query headers may establish recursive filter assignments:

```dsql
query Administration(
  %includeDeleted = false
  %publishedOnly = false
  filter SoftDelete when not %includeDeleted
  filter Published when %publishedOnly
) {
  projects {
    id

    recordings {
      id
    }
  }
}
```

Filter assignments may be interleaved with definition input refinements in the
same header. The refinements remain part of the inferred public input contract;
they do not alter filter matching or enforcement semantics.

An operation assignment applies to every matching query-authored source in its
semantic tree, including:

- root and nested selections;
- selections contributed by fragments;
- aggregate sources and scalar aggregate predicates;
- relationship and table sources traversed by query predicates;
- split-fetch operations derived from the query.

A source-local assignment overrides the operation assignment for that source
only. Descendants continue to inherit the operation assignment unless they have
their own local assignment. An operation assignment matching no source is a
diagnostic.

Recursive operation state never enters filter-authored predicates. Filter rules
evaluate against raw catalog rows and relationships under the separate rule
evaluation boundary below.

## Row Rules

A `where` rule filters rows from its target:

```dsql
filter ProjectAccess on public::projects {
  apply where true
  where $:can_read_all_projects
    or .project_users.user_id == $:user_id
}
```

An active rule applies wherever an operation observes rows from the target:

- root and nested selections;
- relationship traversal in query-authored predicates;
- aggregate sources and scalar relation aggregates;
- split-fetch and other separately executed selections;
- bounded dynamic predicates and ordering.

Filters form the readable source before explicit query `where`, `order by`,
`limit`, `offset`, and aggregation. A query cannot recover a row removed by an
active filter.

Multiple active row rules matching the same target compose with `and`. A table
without an active filter remains readable when a trusted query author selects
it; filters are constraints, not grants in a default-deny permission algebra.

## Field And Relation Rules

One rule may cover a comma-separated field list:

```dsql
filter UserPrivacy on public::users {
  apply where true

  field email, sessions where AdminOrSelf
  field phone where Admin
}
```

Every named field must resolve on every concrete target where the rule applies.
The condition is evaluated independently for each target row.

When a scalar-field condition is false, the field behaves as SQL `NULL`
everywhere the operation can read it, including:

- projection;
- explicit and dynamic predicates;
- ordering;
- aggregate operands;
- group keys.

Conceptually, every query-authored use reads the same expression:

```sql
case
  when can_read_email then users.email
  else null
end
```

This prevents a caller from learning a hidden value through filtering,
ordering, grouping, or aggregation. An unauthorized comparison such as `.email
== %guess` compares `NULL` with the guess and never confirms the hidden value.

When a relation-field condition is false:

- a singular relation behaves as `null`;
- a collection relation behaves as `[]`;
- `exists` yields `false`;
- `count` yields `0`;
- predicate traversal observes an empty relation.

A conditionally readable scalar or to-one relation is nullable in the generated
result type. A to-many relation remains an array but may be empty. Multiple
active rules for the same field compose with `and`.

The initial metadata contract is deliberately conservative across resolution
scopes. Fragment artifacts have one project-wide identity and may be reused by
consumers whose visible filters differ, so a scalar or to-one relation is
nullable when any compiled project filter can mask it on that target. SQL
enforcement and trusted-context parameters remain precise to the operation's
effective scope. A future scope-specific artifact model may safely narrow such
`T | null` contracts back to `T`; this is a non-breaking refinement for
read-only results.

## Rule Evaluation Boundary

Query-authored expressions operate on the filtered logical readable view. This
includes ordinary predicates, dynamic inputs, aggregate operands, and relation
traversal.

A null test on a conditionally readable field also requires that field's access
condition. When access is denied, neither `is null`/`== null` nor `is not
null`/`!= null` matches the row. Results still expose the hidden value as
`null`; the predicate guard prevents callers from distinguishing that mask from
a database `NULL` by probing generated operation inputs.

A filter rule evaluates against raw catalog rows and relationships. It does not
recursively observe field masking or row filters while deciding its own
condition. This lets a rule inspect authorization fields and tables without
becoming self-referential or creating cycles between filters.

The checked semantic plan must preserve the distinction:

```text
filter rule expression   -> raw catalog view
query-authored expression -> filtered logical view
```

## Trusted Context

Trusted request context uses `$:<name>`:

```dsql
$:user_id
$:tenant_id
$:is_admin
$:tenant_ids
```

Context values must resolve to explicit scope-visible DSQL declarations defined
by [`variables.md`](variables.md). Those declarations provide the authoritative
scalar or collection type; filters and conditions consume them but never
declare or infer them.

Rules:

- Context is supplied by a server-side adapter or request boundary.
- Context is never part of public generated operation input.
- Missing required context refuses execution rather than binding `NULL` or a
  caller-provided fallback.
- Explicit query predicates may use context. Every `$:` binding crosses the
  same server-only boundary whether it originates in a filter or directly in
  query source; no project switch can turn it into public operation input.

Roles and capabilities do not need a second policy language. When useful, they
are typed trusted context consumed by filter expressions and conditions.

## Match Lock

Structural targets are convenient but can begin or stop matching after schema
changes. A human-reviewable lock at `<project-root>/dsql/dsql.lock` records the
semantic catalog matches accepted by the project. References to `dsql.lock`
below mean that exact path.

Illustrative shape:

```yaml
version: 1
filters:
  - scope: frontend
    defined_in: shared
    name: SoftDelete
    conditions:
      - scope: shared
        name: PreventReadDeleted
    matches:
      - target: public.projects
        fields:
          deleted_at: timestamptz
      - target: public.users
        fields:
          deleted_at: timestamptz
```

The lock records semantic inputs rather than catalog entity identifiers or a
hash of the entire schema:

- effective resolution scope and filter identity;
- qualified target;
- matched field names and logical types;
- referenced conditions;
- matched relationships when structural relation matching is supported.

`filters` is a sorted list rather than a map because independent scopes may
declare the same filter name. `scope` is the consumer scope whose effective
import closure exposes the filter; `defined_in` preserves the provider
identity. Entries are deterministic and sorted. All filters, including
concrete-target filters, are recorded so the file remains one audit manifest
for resolved filter application. Unrelated catalog changes do not invalidate
it.

A project with no effective filters does not require an empty lock file. An
existing lock that still records removed filters is stale and must be updated.
An unlocked update removes the file when it contains no pinned decisions.

Lock behavior follows package-manager conventions:

- `dsql lock` resolves filters and updates only `dsql.lock`.
- `dsql generate` uses current matches and updates the lock after successful
  compilation before publishing generated artifacts.
- `dsql generate --locked` requires an existing exact lock and never modifies
  it.
- `dsql validate --locked` performs the same read-only match check.
- A missing or stale lock in locked mode is a blocking diagnostic with a
  semantic diff and an instruction to run `dsql lock` or unlocked generation.

The daemon owns resolution, comparison, and writing. `dsql daemon --locked`
selects locked behavior for the process; `dsql daemon` is unlocked. This is a
process invocation choice and does not add lock state to the line-JSON
protocol.

The Vite integration exposes `locked: true | false | "build"`, defaulting to
`"build"`. `true` always spawns a locked daemon, `false` always spawns an
unlocked daemon, and `"build"` uses unlocked mode for the development server
and locked mode for Vite's build command. The TypeScript integration never
reimplements matching or edits the lock directly.

## Aggregates, Fragments, And Split Fetches

Filters attach to resolved catalog observations, not source spelling:

- Aggregates operate over filtered sources and operands.
- Group keys use filtered logical fields.
- Scalar relation aggregates observe filtered relations.
- Fragment expansion preserves local and operation-wide filter assignments.
- Split-fetch execution re-authorizes the complete parent chain and applies the
  same filters as an equivalent inline selection. It preserves trusted-context
  requirements but re-binds their current values at the child request boundary.

These rules belong in the checked semantic plan. SQL generation, metadata
assembly, code generation, and integrations consume that plan and must not
independently rediscover filter applicability.

## Metadata And Tooling

Generated metadata exposes consumer-oriented filter behavior rather than
compiler IR:

- required trusted context and logical types;
- applicable filters, resolved targets, and source provenance;
- declaration defaults, enforcement conditions, and query assignments;
- selected fields whose values are conditionally filtered;
- whether conditional access is context-only or row-dependent;
- source spans for diagnostics, hover, and explain output.

The useful access classification is:

```text
unconditional
conditional: context-only
conditional: row-dependent
```

Each operation policy record is identified by its effective consumer `scope`,
declaration `defined_in`, and `name`, matching the identity shown in
`dsql.lock`. It includes the declaration default and enforcement class,
referenced conditions, required context paths, declaration source map, and its
effective applications. Applications record the result/source `path`, resolved
catalog `target`, assignment state (`default`, `enabled`, `disabled`, or
`conditional`), whether rows are filtered, and affected fields. A disabled
application has `rows_filtered: false` and no affected fields.

Every generated result field has a required `access` value:

- `unconditional`;
- `context_only`;
- `row_dependent`.

This pre-alpha contract replaces the earlier placeholder policy metadata and
makes result access required. There is no compatibility reader for those
prototype artifact shapes; maintained consumers must regenerate metadata and
the generated TypeScript mirror together.

When multiple rules or wrapper effects apply, metadata records the most
restrictive classification (`row_dependent` over `context_only` over
`unconditional`). Policy records sort first by their earliest application path
and then by declaration identity; applications sort by path, target, and
assignment so identical compiler inputs serialize identically.

Framework-specific access helpers are code-generation decisions. Metadata may
support context-only helpers or table-column descriptions without making such
helpers part of the core runtime. A row-dependent rule generally cannot be
answered before querying, especially when it traverses database relations.

LSP hover on a filter or condition shows its declaration scope, resolved
targets, default/enforcement state, and the lock identity in the form
`consumer <- defined_in::name`. Go-to-definition follows references in policy
declarations and query assignments. Completion offers all visible filters in a
query header and only filters matching the current collection in a collection
clause. Generated metadata is the initial machine-readable explain surface; a
separate explain command may be added if consumers need one.

### Deferred Result Access Masks

The initial result contract intentionally does not distinguish a database
`NULL` from a value hidden by a field filter:

```json
{
  "email": null,
  "sessions": []
}
```

`email` may be database-null or filtered, and `sessions` may be empty or hidden.
A UI might eventually want to render "Not provided" differently from
"Restricted", avoid treating a filtered null as editable, or distinguish an
empty relation from one the caller cannot observe.

Supporting that distinction requires per-result information for row-dependent
rules, such as an out-of-band access sidecar:

```json
{
  "result": {
    "email": null,
    "sessions": []
  },
  "access": {
    "email": "filtered",
    "sessions": "filtered"
  }
}
```

This is deferred. It enlarges payloads and public result contracts and could
itself reveal that protected data exists. Static metadata may classify a field
but cannot identify the reason for a particular row's `NULL` or empty relation.

## Predicate Requirements

Filters reuse the core predicate language. It must support:

- `in` and `not in` over literals, public arrays, and trusted context arrays;
- unary boolean `not`;
- `is null` and `is not null`;
- SQL-style related and unrelated `exists` sources.

Existence is a predicate rather than a scalar pipe transform:

```dsql
filter AdministrativeAccess on public::projects {
  apply where true
  where exists public::administrators(
    where .user_id == $:user_id
      and .tenant_id == ..tenant_id
  )
}
```

Filter-authored `exists` observes raw catalog rows under the rule evaluation
boundary. It may use `where` but cannot assign another filter. Query-authored
`exists` observes the filtered logical view and may use ordinary query filter
assignments.

## Non-Goals

The initial filter system does not add direct equivalents of runtime GraphQL
schema permissions such as:

- aggregate allowlists;
- column allowlists for trusted query source;
- query-root field lists;
- role-specific schemas;
- a separate relation-permission model;
- directive-based capability requirements.

A fixed operation selecting a field, relation, root, or aggregate is already an
explicit exposure decision by its author. Filters constrain the data observed
by that operation and its public inputs.

## Open Questions

- Whether reusable conditions should later reference other conditions, accept
  parameters, or describe relationship requirements.
- Whether a future lexical block should assign a filter to a subtree between
  operation-wide and source-local scope.
- Which additional inferred compiler decisions, beyond filter matches, should
  eventually share `dsql.lock`.
