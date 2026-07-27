# Drive per-type behaviour from capability records

**ID:** b50babe0 | **Status:** Done | **Created:** 2026-07-26T11:48:49+02:00

Per-type semantics are spread across exhaustive matches in unrelated modules:
legal operators and literal compatibility in the catalog, `like` support gated on
`DataType::Text` in catalog lookup, aggregate result typing in the aggregate
entity, type-name parsing in the policy entity, and default compatibility in the
variable entity. The TypeScript renderer and runtime repeat the same knowledge in
their own switches.

Adding one type therefore means editing every match arm, and no single place can
state what a type supports. Nothing in this arrangement can be populated from
introspection.

Move operator legality, literal compatibility, orderability, aggregate
eligibility, and the human-readable type description onto a capability record
held by the catalog type arena. The hardcoded mapping stays the sole populator in
this step, so behaviour is unchanged, but it becomes one table in one file.

Sequenced after 1274316f; sequenced before 4b1e4216.

Acceptance criteria:

- Operator legality, literal compatibility, and aggregate eligibility read from
  the capability record.
- `column_is_searchable` tests a `like` capability rather than `DataType::Text`.
- The policy entity resolves type names through the catalog rather than parsing
  them, and an unresolvable name is a diagnostic rather than a silent `Unknown`.
- No exhaustive `match` on `DataType` remains outside the table that builds
  capability records.
- Existing checking, plan, SQL, and metadata snapshots are unchanged.

Resolved by attaching compiler-owned fallback capabilities to each catalog type
and routing predicate checks, dynamic operators, aggregates, policy type names,
searchability, and input defaults through that record. The catalog fingerprint
tracks capabilities only for referenced types. Provider-populated capabilities
remain sequenced under 4b1e4216.
