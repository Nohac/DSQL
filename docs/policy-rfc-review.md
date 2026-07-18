# Policy RFC Review

Date: 2026-07-18

Status: superseded as design guidance. This document records the review of the
earlier multi-concept policy RFC; the accepted filter-only model and current
open questions are normative in [Filters And Access Rules](spec/policies.md).

Review target at the time: the earlier revision of
[Filters And Access Rules](spec/policies.md).

## Verdict

The policy RFC has a strong ergonomic foundation, but it is not ready for
implementation. It can exceed Hasura's read-permission expressiveness through
typed context, capabilities, one predicate language, and stable result shapes.
Its security semantics are still underspecified, however: the RFC does not yet
define the default posture, policy composition, trusted enforcement boundary,
or non-projection uses of protected fields.

The principal problem is that default filters, authorization checks, and query
predicates currently collapse into one `AND` expression without defining
grants, default denial, multiple-policy composition, or override enforcement.

This review concerns read policies. Mutation permissions, presets, and
pre-/post-write checks remain a separate future design.

## Hasura Read-Permission Parity

For read permissions, Hasura supports row filters, column allowlists,
aggregation permission, response limits, relationship traversal, root-access
restrictions, computed-field permissions, session values, and inherited roles.

| Capability | Current RFC |
| --- | --- |
| Typed request/session context | Good proposal |
| Row predicates | Conceptually covered |
| Relationship predicates | Covered for related tables |
| Unrelated-table `exists` checks | Missing |
| Array context and membership | Missing |
| Column permissions | Conditional projection masking only; not equivalent |
| Filter/order/group/aggregate field permissions | Missing |
| Aggregation permission | Missing |
| Policy row limits | Missing |
| Root-only versus relation-only access | Missing |
| Capability or role inheritance | Aspirational; composition is undefined |
| Computed-field permissions | Not defined |
| Trusted server enforcement | Missing |

The audited Hasura corpus uses aggregation permissions, relationship-only
exposure through disabled roots, computed fields, differing role permissions,
and request-dependent row filters. These are practical parity requirements.

References:

- [Hasura access-control patterns](https://hasura.io/blog/access-control-patterns-with-hasura-graphql-engine)
- [Hasura response limits](https://hasura.io/learn/graphql/hasura-advanced/security/5-response-limit/)
- [Hasura root-field restrictions](https://hasura.io/blog/disable-query-subscription-root-fields-in-hasura)
- [Hasura inherited roles](https://hasura.io/blog/cascading-permissions-with-inherited-roles-in-hasura)

## Specification Blockers

### Define the policy algebra

The model should distinguish four concepts:

- `default filter`: a mandatory structural constraint, composed with `AND`.
- `allow`: an authorization grant; applicable grants compose with `OR`.
- `visible when`: conditional projection masking.
- `requires`: a static capability gate enforced before SQL execution.

The effective row predicate should be:

```text
AND(default filters and mandatory constraints)
AND OR(applicable allow predicates)
AND explicit query predicate
AND bounded dynamic predicate
```

No applicable `allow` means denied. This gives default-deny behavior and maps
naturally to inherited capabilities: possessing more capabilities activates
more grants, widening access through `OR`.

An override should only remove a named default filter. It must never implicitly
grant table access. An explicit `deny` primitive should be deferred because its
precedence under capability composition is difficult to reason about; mandatory
constraints already provide the restrictive mechanism.

### Make runtime enforcement normative

The current statement that a capability must be checked by "the host or policy
system" is fail-open. Generated execution must have a mandatory trusted-context
boundary:

```ts
materialize(payload, publicVariables, trustedContext)
```

The contract must guarantee that:

- Context comes only from the server adapter's authenticated request boundary.
- Context keys appearing in public variables are rejected.
- Missing required context refuses execution rather than becoming SQL `NULL`.
- Required capabilities are checked centrally before materialization.
- Overrides are structurally tied to their required capabilities.
- Context values or a server-issued context-scope identifier isolate caches.

The host supplies authenticated facts. It should not be responsible for
remembering how each operation enforces them.

### Define every row-observation boundary

Policies must apply wherever a query observes rows, not only root and nested
projections:

- Root selections.
- Nested relation selections.
- Ungrouped and grouped aggregate sources.
- Scalar relation aggregates in predicates.
- Relation traversal inside query predicates.
- Split-fetch child operations.
- Future computed fields and table-valued functions.

A crucial recursion rule is required:

- Query-authored traversal sees policy-filtered related rows.
- Policy-authored traversal operates against raw catalog relations and does not
  recursively apply policies on the target table.

Without this distinction, authorization predicates can recurse through policies
on membership tables or form cycles. Authorization tables also commonly need to
participate in checks without being publicly readable themselves.

Split-fetch child operations must reauthorize the parent chain. Accepting a
parent identity supplied by the client is not sufficient authorization.

### Close field-level side channels

Rendering a hidden field as `CASE WHEN ... THEN value ELSE NULL` protects only
the projection. A protected value can still leak through:

- `where`
- `order by`
- grouped keys
- aggregate operands
- field comparisons
- relation existence
- dynamic filter presets

For the first implementation:

- Conditional fields may be selected as masked projections.
- Conditional fields are forbidden in filters, ordering, grouping, aggregate
  operands, and dynamic filter presets.
- Such use requires a capability that makes access unconditional for the
  operation.
- Dynamic presets such as `selected` or `searchable` exclude conditional fields
  by default.
- An always-denied field produces a compile error instead of an always-null
  result.
- Hidden relations follow the same rules for predicates and aggregates.

### Complete the predicate substrate

Hasura-level policy predicates require at least:

- `in` and `not in`
- arrays or sets in trusted context
- boolean `not`
- null predicates
- unrelated-table `exists`
- typed context declarations and `$:` grammar support

Scoped relationship paths cover most authorization rules ergonomically, but
they cannot express checks against an unrelated authorization table.

### Define structural-target safety

Shape matching is ergonomic:

```dsql
default filter TenantScope on {
  .tenant_id: uuid
}
```

A column rename can silently stop the policy from matching. The specification
should require:

- Applied-target reporting.
- Optional expected match counts or explicit target assertions.
- Diagnostics when an expected target disappears.
- Source provenance for every match.
- Composition, rather than precedence-based replacement, between provider and
  project constraints.

Structural default filters are reasonably safe because newly matching targets
become more restricted. Structural grants are more dangerous and should be
deferred initially.

### Define stable-shape consequences

Stable result shapes are more ergonomic than Hasura's role-specific schemas,
but their consequences must be normative:

- Conditional visibility makes the generated field nullable.
- A masked value is intentionally indistinguishable from a database `NULL`.
- A hidden to-many relation becomes `[]`; a hidden to-one relation becomes
  `null`.
- A row policy makes an otherwise non-null singular relation nullable because
  it can suppress the related row.
- Policy row limits do not prove singular cardinality. Public cardinality must
  not vary with the request's policy environment.
- Aggregate permissions apply before aggregation. Group keys and aggregate
  operands require explicit field-use permission.

## Recommended Compiler Direction

Policies should have one normalized semantic source and three explicit
downstream touchpoints: resolution, planning, and runtime enforcement.

| Stage | Responsibility |
| --- | --- |
| Ingestion | Normalize project and provider declarations into a fingerprinted policy schema |
| Resolution | Resolve targets, context, fields, relations, and predicates once |
| Planning | Apply resolved policies to every table scope and preserve provenance |
| SQL | Mechanically render planned filters, guards, and masks |
| Metadata | Copy context, capabilities, policy provenance, and final result shape from the plan |
| Runtime | Bind trusted context, enforce capabilities, materialize parameters, and execute |

### Ingestion and resolution

Introduce a fingerprinted policy input containing:

- Context schema.
- Default filters.
- Row grants and mandatory constraints.
- Field and relation rules.
- Aggregate, root, and traversal permissions.
- Limits and override capabilities.
- Source provenance.

Resolve this against the catalog into a canonical policy index keyed by stable
table, column, and relationship IDs. Policy expressions should reuse the
existing typed expression and path resolver. They should not use a separate
Hasura-shaped expression engine or synthetic clause entities.

Policies sourced from multiple documents will eventually need a `DefIndex`-like
tracked fingerprint. Ambient policy walks without such an index would recreate
the incrementality problems already solved for fragments.

`ResolvedSelection` remains the semantic authority. It should carry applicable
policy effects, and policy filtering must participate in singular-relation
nullability. Checks, variables, planning, metadata, and editor services must not
independently decide which policy applies.

Before policy work, planning should stop resolving scalar fields again through
the catalog. Every projection and relation should consume the same resolved
selection target.

### Planning

Keep explicit clauses and policy effects separate:

```rust
CollectionPlan {
    clauses: SelectionClauses,
    policy: CollectionPolicyPlan,
    result: CollectionResultPlan,
}
```

Projections and relations should carry their own visibility and capability
effects. Policy filters should not be merged into `SelectionClauses.filter`,
because separate plan fields preserve:

- Provenance.
- Debug and explain output.
- Override validation.
- Capability requirements.
- Tests that distinguish implicit policy filters from explicit predicates.

One policy-aware table-scope constructor should be used for roots, nested
relations, query-predicate `EXISTS`, aggregate sources, and scalar relation
aggregates. This makes a forgotten policy application structurally difficult.

The plan should carry the final scalar data type and nullability for every
projection. Metadata assembly currently derives scalar nullability from the
catalog; once policies affect nullability, that decision must belong to the
operation plan so SQL and metadata cannot diverge.

### SQL generation

SQL generation should remain mostly ignorant of policy semantics:

- Row predicates apply before ordering, limiting, and aggregation.
- Scalar visibility renders as `CASE`.
- Relation visibility becomes a child-source guard, naturally producing `[]`
  or `null` through the existing result envelopes.
- Context values become parameterized values with a distinct `context.*`
  source.
- Policy row caps combine with public limits through a planned cap, not the
  global SQL collection-limit option.

### Metadata and runtime

Generated metadata must expose:

- Required context fields and types.
- Required capabilities.
- Applied policy identities and sources.
- Matched policy targets.
- Conditional visibility.
- Policy-driven result nullability.
- Parameter source: public input or trusted context.

Generated clients must never accept context values. Generated server adapters
obtain them from authenticated request state and refuse execution when required
context or capabilities are absent.

### Porridge tracking

Policy inputs and the catalog must be tracked dependencies of policy resolution.
A policy edit must retire and rederive every affected resolution, plan, SQL fact,
and artifact. Initially, one fingerprinted policy singleton may invalidate all
policy-aware work; it can later be sharded by target table if measurements show
that to be necessary.

Policy facts derived from source documents need `DerivedFrom` ownership and a
fingerprinted set index. Policy-aware systems must not rely on untracked ambient
views.

Policy correctness is not an optional demand. Planning and SQL generation must
always depend on successfully resolved policies. Diagnostics demand controls
reporting work, but an invalid policy configuration must block artifact
generation even when diagnostics were not explicitly requested.

## Suggested Implementation Order

1. Revise the RFC with the policy algebra, default-deny posture, traversal
   rules, side-channel rules, and trusted runtime contract.
2. Make planning consume resolved scalar selections and carry final projection
   type and nullability.
3. Add typed `$:` context through grammar, resolution, parameters, metadata,
   and server-only execution.
4. Implement concrete-table row policies and default filters for roots and
   nested selections.
5. Cover aggregates, query-predicate traversal, root/relation access, and row
   caps.
6. Implement conditional field/relation visibility and non-projection
   restrictions.
7. Add capabilities and audited overrides.
8. Add editor/debug output and Hasura metadata import.
9. Add structural shape matching after concrete targeting is proven.

The feature should not be considered production-safe before aggregate and
predicate traversal are policy-aware and field side channels are closed.
