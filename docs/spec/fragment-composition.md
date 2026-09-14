# Explicit Fragment Composition

Status: proposed implementation contract; not implemented. Updated 2026-09-14.

This specification defines `...(A and B)` and `...(A or B)` as explicit
composition of fragment selections. It supplements ordinary fragment spreading;
it does not change the collision rules of adjacent ordinary spreads.

The contract composes matching selections independently at each output path,
including each operand's independently filtered, ordered, and paginated rows.
Whole-fragment branch membership is outside this version.

## Purpose

Independently authored fragments often select the same scalar or relationship.
A query author should be able to combine those contributions without editing
the fragments or implicitly accepting different query logic.

Both operators combine requested fields. `and` intersects the row identities
returned by matching relation selections; `or` includes identities returned by
either selection, once each. The set operation is on source-row identities,
not on projected JSON objects or entire fragment result types. SQL backends may
use joins, existence checks, or set operations to implement that contract.

```dsql
fragment PublishedPosts on users {
  posts(where .published == true) { id title }
}

fragment RecentPosts on users {
  posts(where .created_at > %since) { id created_at }
}

query Users {
  users {
    ...(PublishedPosts and RecentPosts(%since))
  }
}
```

The composed `posts` has the fields `id`, `title`, and `created_at`, once each.
With no pagination or generator caps, its membership can be implemented using
the query-authored predicate `.published == true and .created_at > %since`.
The explicit binding forwards the second fragment's param to the query's
`params.since`. With `or`, the predicate uses `or` and the fields are unchanged.
Fragments remain independently checked contributions; the composition site
explicitly changes the combined row requirements.

## Syntax And Placement

```dsql
...(PublishedPosts and RecentPosts(%since))
...(PublishedPosts or FeaturedPosts)
...((PublishedPosts and RecentPosts(%since)) or FeaturedPosts)
...(PublishedPosts and (RecentPosts(%since) or FeaturedPosts))
```

The grammar extension is:

```text
composition_spread = "..." "(" composition_or ")"
composition_or     = composition_and ("or" composition_and)*
composition_and    = composition_term ("and" composition_term)*
composition_term   = fragment_operand | "(" composition_or ")"
fragment_operand   = fragment_name [binding_list] directive*
```

`and` binds more tightly than `or`. Operators are lowercase, matching predicate
keywords. Grouping parentheses override precedence. A single operand, `...(A)`,
has the behavior of `...A`; an empty expression is invalid. Operands do not
repeat `...`. There are no comma, implicit-conjunction, `not`, inline-selection,
or query-name operands. Existing fragment-name and binding-list grammar applies;
an empty binding list is still invalid.

Every operand resolves as a fragment in the composition site's normal resolution
scope. Its declared table must match that site's current object context. A
composition is permitted inside a relation selection body or a fragment body,
including extracted DSQL documents. It is not a query-root selection: fragments
do not introduce a root table. A fragment body may contain further composition
spreads; normal visibility, compatibility, and cycle diagnostics apply.

Operand directives have ordinary spread-site ownership. The outer composition
has no new directive location in the initial version. Unsupported existing
directive semantics remain unsupported.

## Composition Boundary

An explicit group produces one checked set of output selections. Existing
collision checks then compare it with neighboring direct selections, ordinary
spreads, and other composition groups. A group does not absorb its neighbors:

```dsql
users {
  id
  ...(Identity and Profile) # still conflicts if the group also exports id
}
```

The query author must put the intended fragment contributions in the same group
or choose distinct result keys. This feature introduces no general field override
or aliased-fragment syntax.

Each operand's body is checked before composition. An invalid fragment containing
duplicate output keys, bad bindings, unresolved references, or a cycle cannot be
repaired by another operand. Every operand is validated even when its predicate
would make another branch redundant.

## Per-Path Selection Algebra

At each object scope, project the composition expression onto each output key.
An operand either contributes one independently valid selection at that key or
has **no contribution** there. Evaluate the original grouping with these rules:

| Left | Right | `and` or `or` result |
| --- | --- | --- |
| No contribution | No contribution | No selection |
| Selection | No contribution | That selection, unchanged |
| No contribution | Selection | That selection, unchanged |
| Selection | Selection | Check compatibility and combine as below |

No contribution is not a boolean predicate. In particular, do not distribute
operators across the fragment expression before projecting it onto output keys.
If `B` omits a key, `A and (B or C)` gives `P_A and P_C` there, while
`(A and B) or (A and C)` gives `P_A or (P_A and P_C)`. The expressions are not
interchangeable. The compiler preserves the authored tree and does not expand
it into disjunctive or conjunctive normal form.

### Scalar Fields

Matching scalar output keys merge only when they resolve to the same source
column in the same object context and have compatible effective access and
directive semantics. Emit the scalar once. Different aliases remain distinct
keys even when they reference the same column. Two different columns under the
same alias are a blocking conflict; neither operator chooses a winner.

### Relation Fields

Matching keyed relation selections must resolve to the same catalog target,
relationship edge/direction, correlation context, and compatible effective
policy/directive context. Matching table spellings or output names are not
sufficient. Compare resolved identities, not source strings or SQL aliases.

Evaluate each operand's complete row selection, including its window, and combine
the returned row identities with the chosen operator. Without windows this may
reduce to combining query-authored predicates. Recursively compose child selections
with that same operator. Non-overlapping children keep their own predicates and
shape. A child selection is evaluated for every row
returned by its composed parent; it is not gated by which parent operand matched.

For example, if `A` contributes `posts(P) { comments(C) { id } }` and `B`
contributes `posts(Q) { comments(D) { body } }`, with no windows or caps, `A or B` means:

```text
posts:    P or Q
comments: C or D, for every returned post
fields:   comments.id and comments.body
```

It does not mean “use C only when P matched, and D only when Q matched.” The same
rule applies to scalar projections: fields contributed by either operand are
projected for all admitted rows, subject to their field-access guards. Query
filters are not field authorization. Branch-correlated views require a separate
future contract.

### Unfiltered, Optional, And Null Predicates

An explicitly selected relation with no `where` has the query predicate `true`.
This differs from not selecting that relation:

```text
unfiltered posts and posts(P) -> posts(P)
unfiltered posts or  posts(P) -> unfiltered posts
no posts contribution or posts(P) -> posts(P)
```

These reductions assume no pagination or generator caps. An unfiltered selection
with `limit 10` contributes its ten-row window, not every source row.

First apply the existing optional-input pruning rules independently within each
contributed selection's complete `where`. If that entire predicate becomes
absent, the selection is unfiltered and contributes `true` at the composition
boundary. Only then apply composition `and`/`or`.

Thus unsliced `posts(where .id == %id)` with a nullable `%id = null`, composed
using `or` with unsliced `posts(where .published == true)`, becomes unfiltered
when `%id` is null, assuming no generator cap.
Splicing those two predicate ASTs into the existing optional-expression walker
would incorrectly retain only the published predicate. The compiler must retain
the complete-selection boundary through planning and SQL rendering.

SQL null/unknown is still normal three-valued predicate behavior; a database
NULL does not mean structural absence. Public bindings are validated and
materialized for all operands before execution. Boolean simplification does not
remove required inputs, defaults, diagnostics, or provenance from the public
contract. It may remove unused SQL parameter slots once the checked public
contract has been retained.

## Row Identity, Ordering, And Pagination

Every matching ordinary relation selection being composed requires a catalog
proof of row identity: a primary key or a non-partial, non-expression unique key
whose columns are all non-null. Composite keys are supported. An authored view
uniqueness proof may qualify under the existing overlay contract. Partial keys,
nullable unique keys, selected-column equality, generated row numbers, `ctid`,
and SQL aliases do not establish identity for this feature. A missing proof is
a blocking diagnostic, even if a particular unsliced query could be optimized
without one. Optimizer choices must not change which programs are accepted.

The key must also have representable PostgreSQL equality and ordering semantics
for membership tests and deterministic tie-breakers. Preserve the key's comparison
semantics, including relevant collation/operator-class requirements. If current
catalog facts cannot establish a usable comparison contract, reject that key
rather than assume an unrelated default comparator proves identity.

Prefer the primary key; otherwise choose the eligible key by stable
lexicographic column-name tuple. Identity follows the source row, using raw key
columns internally after row-policy filtering. Key columns need not be selected
or publicly readable. They never appear as extra result fields or public inputs;
projected key values still follow field masking. A mask cannot collapse two
distinct row identities. All candidates run in the same SQL statement snapshot.

### Complete Operand Windows

For each matching relation and each parent row:

1. Resolve its correlation and effective readable view.
2. Apply that operand's complete query predicate, including optional pruning.
3. Apply its own ordering, then offset and effective limit using existing rules.
4. Retain the resulting ordered source-row identities for composition.

To make each composed operand's window deterministic, append the identity key
columns ascending as tie-breakers to its authored order. If it has no ordering,
use identity-key order. This stabilization applies within a composed matching
selection; non-overlapping selections keep their ordinary ordering contract.
Authored order expressions read the masked view; hidden identity tie-breakers
remain internal. Nullable ordering/pagination first follows existing removal
rules, then the deterministic window order is established.

An `and` or `or` composes the already computed windows. It must not replace them
with one window over combined predicates. “Newest ten published rows intersected
with newest ten recent rows” is distinct from “newest ten rows both published
and recent.” Unequal limits, offsets, ordering directions, and independent input
bindings are permitted and retain their own meaning.

For example, assuming `posts.id` is a primary key:

```dsql
fragment PublishedPage(%count = 10 %skip = 0) on users {
  posts(where .published == true order by created_at desc
        limit %count offset %skip) {
    id title
  }
}

fragment RecentPage(%count = 5) on users {
  posts(where .created_at > %since order by id asc limit %count) {
    id created_at
  }
}

query Users {
  users {
    ...(PublishedPage(% <- %published) or RecentPage(% <- %recent))
  }
}
```

`params.published.count`/`skip` and `params.recent.count`/`since` belong to their
respective operands. This query returns the union of those independently
computed pages, not a new ten- or five-row page over the combined predicate.

### Result Order

Composition uses explicit left-to-right priority:

- `A and B` retains A's order and keeps only identities also returned by B.
- `A or B` returns A's ordered identities, followed by identities returned only
  by B in B's order. An identity appearing in both occurs once at its A position.
- Nested expressions apply those rules at each binary node, preserving grouping.

For `A = [5, 4, 3]` and `B = [4, 2, 1]`:

```text
A and B -> [4]
A or B  -> [5, 4, 3, 2, 1]
B or A  -> [4, 2, 1, 5, 3]
```

Membership is commutative where both operands contribute; result order is not.
There is no implicit final sort, limit, or offset after composition. This is
ordered set union/intersection over source identity, not `UNION ALL` of output
objects. The merged projection and nested child contributions are evaluated
once for each final admitted source row.

Order must be stable between compiler generations for unchanged query semantics,
bindings, effective catalog, and database contents. Generation numbers, file
discovery order, scheduler order, and internal entity IDs must not affect operand
priority or tie-breaking. Warm and cold compilation, harmless formatting changes,
and source-location moves preserve row order. Explicitly reordering operands,
changing their clauses/inputs, or changing the underlying rows can change it.

The generator's existing collection cap applies to each operand's effective
window exactly as it would for that selection alone. No additional final cap
silently discards the composed result. An OR of two capped windows can therefore
return up to the sum of their bounds; AND is bounded by their minimum. Runtime
caps do not establish new static singular-result proofs. Resource limits may
reject an operation explicitly, but must not change these windows by truncation.

### Safe Optimizations And Unsupported Shapes

Normalization resolves catalog/input identities, makes existing syntactic defaults
explicit, and ignores source locations. Structurally identical complete windows
may be evaluated once with merged projections. This proof includes predicates,
ordering, limit/offset, effective policy contracts, and optional boundaries; equal
defaults on independent inputs or equal values in one request are not proof.

When neither operand has an effective window, membership may lower to a combined
predicate if the backend also preserves the specified priority order. For OR,
one plain globally sorted scan is generally insufficient: A-matching rows must
precede B-only rows. Every optimization must preserve both membership and order.

Overlapping aggregate transforms and flattened exports remain unsupported in
the initial version. Non-overlapping aggregates and flattened selections pass
through with their existing validation and output-key provenance. A flattened
output must not become a plain scalar candidate through loss of its provenance.
Use distinct result aliases for incompatible source/access/transform contracts.
This version adds no post-composition window or aggregate-composition syntax.

## Bindings And Input Identity

Each operand is an ordinary spread occurrence for input inference. Apply
containment, whole-root lifting, namespaced lifting, or explicit leaf mappings
using [Variables](variables.md#bound-definition-inputs) before comparing or combining
selections. Grouping and operator choice introduce no input namespace segment.

```dsql
...(RecentPosts(%since <- %first_since)
    or RecentPosts(%since <- %second_since))

...(RecentPosts(% <- %first) or RecentPosts(% <- %second))
```

In the first example the two predicates retain distinct caller inputs. In the
second the params roots remain separate under `params.first` and `params.second`.
All required leaves in an explicitly bound root still need coverage.

Do not infer new inputs from the merged result path. Public input identity comes
from the checked spread rewrite and caller binding, including its complete type,
role, nullability, default, and validation contract. The same public binding may
be used by several predicates; equal defaults on distinct bindings do not make
them the same input.

Repeated operands with the same contained path and compatible contracts share
that public path under the existing contract-merging rule. Independent values
require explicit distinct leaf bindings or named roots; the compiler invents no
occurrence-number namespaces. Source occurrence identity and provenance remain
distinct even when public input paths coincide.

Trusted `$:` inputs remain separately declared and operation-global. No binding
list can turn them into public inputs. Bounded dynamic inputs owned by fragments
remain unsupported in this initial extension. An enclosing query's `selected`
capability preset sees the final checked projected field set, with duplicate
scalars present once and existing policy restrictions intact.

## Policies, Cardinality, And Result Types

Composition cannot combine incompatible readable views. For a matching selection,
the participating effective row-policy set, field/relation guards, application
conditions, and directive effects must be structurally compatible after input
rewriting. Policy identities participate; source spans do not. An `or` never
chooses the more permissive policy branch or discards a guard.

For unsliced selections without generator caps, a shared policy predicate `S`
and query predicates `P` and `Q` admit these membership reductions:

```text
and: S and (P and Q)
or:  S and (P or Q)
```

With windows, enforce `S` inside each candidate before its own sort and slice;
these reductions cannot move the window to a shared outer predicate.

Apply field masking consistently to all query-authored reads, as required by
[Filters And Access Rules](policies.md). Operation-wide assignments retain their
existing inheritance. Composition creates neither a new assignment scope nor a
new filter match-lock entry. Existing effective filter matches still determine
`dsql.lock`. Different source-local assignments are conflicts unless their
checked effective contracts satisfy the compatibility rule.

Recompute cardinality and nullability for each composed selection from its
membership expression, catalog relationship proofs, and operand windows. For a
collection relation, `.id == %first or .id == %second` cannot inherit the two
operands' separate at-most-one proofs. Intersection is at most one when either
operand has a valid at-most-one proof; union can contain two distinct rows even
when each operand has `limit 1`. A catalog-singular relationship or proof of the
same unique identity may keep union singular. Use a conservative collection shape
when proof is absent, and retain the existing distinction between literal limits
and runtime pagination. An empty singular intersection is null, not an empty
object; an empty collection result is `[]`.
Parent absence and field-policy nullability still propagate normally.

The result has one fixed, combined field shape; it is not a union of fragment
object types selected at runtime. Standalone fragment result types describe
their standalone plans and cannot be reused blindly for composed occurrences.

## Metadata And Versioning

When implemented, this changes the artifact contract. Bump
`BUILD_MANIFEST_VERSION` from 7 to 8 (or the next version if another contract
change lands first), regenerate TypeScript/schema outputs, and reject older
versions. Do not add compatibility fallbacks. The documentation change itself
does not bump the running compiler's version.

Operation and fragment metadata gain a required `fragment_compositions` array;
ordinary artifacts emit `[]`. Each record contains:

- `id`: an artifact-local, deterministic composition occurrence ID;
- `path`: the result-object path at which the composition is applied, using the
  existing spread path convention;
- `expression`: a tagged tree with `kind: "and"` or `"or"` and `left`/`right`,
  or `kind: "fragment"` with `name`, resolved provider `scope`, and `source_id`.

`source_id` references an entry in the existing `source_map` naming the operand's
source file and range, including host projection when embedded. Group IDs also
reference source-map entries for the complete group. IDs follow deterministic
source/expansion preorder, remain unique for repeated expanded occurrences, and
never serialize Bowl entity IDs. Whitespace edits may change ranges and IDs;
stability is required for identical settled inputs, not across arbitrary edits.
Every nested or fragment-inherited composition is included at its effective
path. Existing `fragment_spreads` remains the occurrence provenance list; it does
not encode the composition operator tree.

Result fields come exclusively from the final checked composed plan. In the
initial renderer, an artifact with nonempty `fragment_compositions` renders its
result structure from those fields without substituting standalone fragment
result types. This conservative rule also covers composition inherited through
an ordinary spread. Future reuse requires proof that the use-site shape matches.
Input types and sparse scalar codecs continue to follow the artifact's actual
input/result metadata.

Composition is compiled before execution. Runtimes consume generated SQL,
parameters, variants, and result contracts; they do not merge fragment queries.
The daemon's protocol framing need not change solely for these versioned
metadata fields, but every metadata consumer must validate the bumped version.

## Diagnostics, Services, And Determinism

Use existing unknown/ambiguous fragment, binding, and cycle diagnostics for
operand errors. Composition-specific conflicts identify the output path, both
contributing source sites, and the incompatible dimension: source, policy,
directive, row identity, or result kind. Reject unsupported placement and
composition shapes explicitly. A failed group blocks the containing artifact;
planning/SQL must not silently drop one operand or publish a partial selection.

Completion within a group offers compatible fragments and the legal operators;
binding completion keeps ordinary spread behavior. Hover and definition target
the individual operand. Hover on the group explains its operator tree, combined
paths, and window ownership using the same checked composition result. Formatting
preserves grouping, operand bindings, directives, and comments; malformed input
retains the existing conservative behavior. Semantic tokens reuse the existing
fragment/operator classifications.

Output fields use first appearance in a left-to-right traversal, recursively;
later compatible occurrences do not move an existing key. This ordering rule
does not imply database row ordering. SQL parameter allocation and provenance
are deterministic for the same input program. Reordering operands may reorder
output fields and parameter slots while preserving admitted rows.

Warm and cold compilation must agree after operand edits, binding changes,
operator/grouping changes, provider moves/deletions/reinsertions, and policy or
catalog changes. Removing the last group must retire its checked facts and
metadata. Shared input or predicate nodes must not erase occurrence provenance.

## Required Conformance Cases

| Case | Required observation |
| --- | --- |
| C01: scalar overlap, both operators | One copy of each compatible scalar, stable field order |
| C02: same relation with P/Q | Intersect/union complete candidate identities; matching both appears once |
| C03: omitted versus unfiltered relation | Omission is no contribution; unfiltered is `true` before its window |
| C04: nullable operand removes a complete `where` | `or` becomes unfiltered; SQL unknown remains distinct |
| C05: mixed operators, grouping, missing keys | Preserve projected expression grouping; do not distribute |
| C06: nested relations | Per-path child predicates apply to every returned parent |
| C07: independent, shared, namespaced, repeated bindings | Exact input paths/defaults remain intact; conflicting contracts reject |
| C08: aliased/source conflicts and outside neighbors | Different keys remain separate; incompatible same keys reject |
| C09: ordering/pagination | Different filtered/ordered/offset windows compose by identity in left-priority order; no final truncation |
| C10: policies and directives | Enforced guards survive `or`; incompatible readable views reject |
| C11: cardinality changes | Combined SQL envelopes and generated types agree, including unique-key OR |
| C12: aggregates, flattening, dynamic surfaces | Unsupported overlaps reject; non-overlaps pass through; outer presets use final fields |
| C13: invalid/cyclic operands and unsupported placement | Stable diagnostics, no partial artifacts, no short-circuit acceptance |
| C14: source edits and lifecycle | Warm/cold equality and stable row order across generations/formatting/moves; no stale rows; unrelated roots stay asleep |
| C15: generated/runtime/editor integration | Strict version, truthful result types/codecs, correct source maps and operand services |
| C16: identity proofs | Composite keys work; missing/nullable/partial or unusable-comparator proofs reject; hidden/masked keys stay private |
| C17: windows and optional inputs | Null removes only its own clause; ties use identity order; caps apply per operand; empty windows compose correctly |

These cases are acceptance requirements for implementation, not claims of current
test coverage. See the [implementation plan](../plans/fragment-composition.md).
