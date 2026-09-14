# Fragment Composition Implementation Plan

Status: ready for implementation planning review; implementation not started.
Updated 2026-09-14.

Contract: [Explicit Fragment Composition](../spec/fragment-composition.md).
Tracking issue: [abae32e5](../../issues/abae32e5-implement-explicit-fragment-composition.md).
The spec owns language behavior. This plan owns sequencing, compiler boundaries,
and verification. This documentation batch changes no compiler behavior.

Confirmed design choices: include independently paginated results in the initial
contract, compose nested relations per output path, and use left-to-right result
priority. Stable ordering across generations is the primary ordering requirement;
the compiler must not derive precedence from discovery, scheduling, or entity IDs.

## Target And Scope

Implement `...(A and B)` and `...(A or B)` with existing operand bindings,
explicit grouping, per-output-path composition, strict policy compatibility,
independently paginated operand windows, and truthful result types. Deliver both
operators, including nullable inputs, through SQL, native execution, metadata,
TypeScript, and editor services.

Initial exclusions are the spec's own: query-root composition,
overlapping transforms/flattening, missing reliable row identity, branch-correlated
views, and fragment-owned bounded dynamic inputs. These must produce diagnostics;
they must not be silently approximated. Ordinary spread collision behavior stays
strict outside an explicit group.

No Opus/Fable review is part of this work, as requested. Apply repository Rust
and architecture guidelines directly. Use atomic commits per coherent batch;
do not merge parse-but-ignore support or maintain parallel old/new semantics.

## Current Implementation And Required Changes

| Area | Current behavior | Required change |
| --- | --- | --- |
| `dsql-core/src/grammar/dsql.llw` | A spread names one fragment, with optional bindings/directives | Add composition expressions and operands using existing `and`/`or` tokens |
| `entities/lowering.rs`, `entity.rs`, `schema.rs` | Generic walk assigns structural parents and semantic membership | Represent group membership without changing operand input scope or orphaning syntax |
| `entities/fragment_spread.rs`, `entities/expansion.rs` | Each `SpreadDecl` resolves/expands as one occurrence | Reuse leaf resolution/expansion; preserve group expression and occurrence provenance |
| `entities/variable.rs`, `variable_path.rs` | Effective contracts and per-spread rewrites precede planning | Traverse all operands, retain binding identity, and avoid re-inference from merged output |
| `entities/field_selection.rs` | `check_output_keys` rejects repeated keys; `DefinitionClosure` separates fields/spreads | Recognize group boundaries; validate one checked combined frontier against neighbors |
| `plan/build.rs`, `plan/types.rs` | Planner appends each spread's items and copies local selection shapes | Normalize bound selection contributions, compose, then finalize cardinality and plans |
| `resolution.rs`, catalog keys/metadata | Cardinality proofs operate on each source selection | Resolve proven identity keys and derive bounds of composed windows |
| `sql/postgres.rs` | Renders one source window; optional pruning occurs within one predicate | Render per-operand identity windows, ordered union/intersection, then merged projection |
| `dsql-generate`, `dsql-metadata` | Flat spread provenance and standalone fragment shapes | Export composition tree/source provenance and bump the artifact version |
| TypeScript renderer | Reuses fragment result types based on path/name provenance | Render composition-bearing artifact shapes from final result metadata |
| Core/LSP/tree-sitter services | Understand ordinary spread syntax and source positions | Add operand-aware completion/navigation, group explanations, formatting, and captures |

## Ownership And Data Flow

Use one syntax owner for the composition concept under `entities/`, co-locating
its lowering, formatting, diagnostics, and service contributions. Keep each
operand as a real spread occurrence using the current name-resolution and input
contract machinery. Store the expression as a typed `Fragment`/`And`/`Or` tree
referencing those occurrences; it is not a second copy of fragment bodies.

Composition membership is distinct from ordinary selection ancestry. Merely
making a group the new `ChildOf` parent would hide its spreads from today's
`spreads_under` collectors and shift inferred input scopes. Either retain the
existing parent and add explicit composition membership, or introduce one shared
group-aware traversal that preserves the same semantic parent. Prefer the former
for the first implementation; the checked frontier must exclude operand leaves
from being appended a second time as ordinary spreads.

The intended dependency order is:

```text
source -> syntax/operand sites -> name/catalog resolution + expansion
       -> effective input contracts and operand rewrite maps
       -> bound selection contributions + effective policy contracts
       -> checked composition of identity/window descriptors (or problems)
       -> final cardinality/result shape + query/fragment plans
       -> SQL, metadata, editor explanations
```

Extract the necessary bound-selection construction from `Planner::plan_selection_set`
into a named internal module. Produce checked definition-local selection facts
for all consumers; ordinary selection sets use strict collision mode, while
explicit groups carry their authored operator tree. Query/fragment plan assembly
consumes these facts rather than expanding operands again. Keep the pure
composition algorithm in a named module such as `plan/compose.rs`; do not add a
second catalog resolver, variable inference engine, or SQL generator.

Use a bound descriptor with catalog source/edge/correlation identity, output key,
separate query predicate and policy contract, ordered clauses, projection kind,
children, and contributor provenance. Structural equality excludes spans and
includes resolved input identities. Do not compare rendered SQL or hash equality
alone as proof of compatibility. Preserve the expression tree and evaluate it
per key; no distributive expansion or runtime query merging.

Extend the collection source representation with a typed membership expression
over complete operand windows, retaining one shared result projection. Each
window carries its original checked clauses and source identity proof. Binary
nodes carry ordered intersection/union, not an already flattened `where`.
The descriptor owns the deterministic identity tie-breaker and left-priority
ordering contract. Keep the ordinary one-source representation as a legitimate
variant of the same model, not a compatibility path to retire later.

The checked result must publish a discriminated valid/problem state. A valid
empty or non-overlapping contribution is not interchangeable with an unresolved,
unsupported, or cyclic operand. Consumer plans cannot silently omit failed groups.
Reuse the existing `SelectionPlan`/`FilterExpr` representations where they own
the right knowledge; introduce new types only for the missing composition and
bound-selection contracts.

Follow the current narrow-anchor lifetime rules: a relationship owner is not a
`DerivedFrom` anchor. Keep operator/source navigation payloads separate from
semantic fingerprints where possible. Extend exact semantic membership and
expansion relationships, not global views in every definition's system. Retire
checked outputs after operator removal and in-place deletion of the last group.

Input inference remains upstream of composition; policy applicability starts
from the raw resolved source closure, so neither depends on final composed
output shape. Query-owned `selected` dynamic presets are finalized from checked
projected fields after composition. This avoids an inference/composition cycle
while respecting the existing ban on fragment-owned dynamic inputs.

## Implementation Batches

### 1. Syntax, ownership, and traversal

- Add grammar rules for the outer spread, precedence levels, grouped terms,
  and fragment operands. Reuse binding-list/directive grammar and existing
  keyword tokens. Update both exhaustive lowering and formatting dispatch.
- Define schema shapes and register the composition syntax owner. Retain leaf
  `SpreadDecl` identities, spans, binding lists, and semantic group membership.
- Update `DefinitionClosure`, expansion payloads, and shared traversal so input
  inference, policies, checks, and planning all see every operand exactly once.
- Implement CST formatting with preserved precedence/comments; update tree-sitter
  grammar, surface ownership, highlights, and indentation coverage together.
- Add parse/lowering/format/expansion tests for simple and mixed operators, single
  operands, nested composition, leaf bindings, malformed expressions, and cycles.

Exit: the frontend represents group boundaries without changing existing input
paths. Until semantics land, recognized composition must emit a blocking
unsupported-feature diagnostic. Remove that temporary diagnostic only when
batches 3 and 4 activate the complete versioned contract together; never keep a
hidden flag or parse-only acceptance path.

### 2. Bound selections and checked composition

- Extract bound descriptor construction and strict frontier validation from
  existing planner/checker code. Preserve all ordinary fixture behavior.
- Feed effective operand rewrite maps into the descriptor before comparison.
  Retain the full public contract even if a predicate later simplifies away.
- Implement per-key expression projection with an explicit no-contribution
  state. Combine scalars by compatible identity; recurse for keyed relations.
- Check source, effective policy/directive, and row-identity contracts. Select a
  primary or eligible non-null unique key deterministically, retaining overlay
  proof provenance. Keep each operand's different order/window in its descriptor.
  Check usable native equality/order semantics; extend provider metadata only
  where needed to establish the comparison contract, and reject unproven keys.
  Preserve original flattened/aggregate provenance so forbidden overlaps cannot
  masquerade as compatible scalar fields.
- Emit structured composition problems naming both sites and the result path.
  Replace group-blind duplicate checks with checks against the resulting frontier;
  retain independent operand validation and strict checking against neighbors.
- Publish one tracked checked result per definition for plan assembly and service
  explanations. Define demand propagation so `check`, LSP, `validate`, and generation
  all request the binding and composition prerequisites. Low-level diagnostic
  requests encountering a group must also obtain its semantic prerequisites;
  do not rely on caller insertion order or silently skip checks without variables.

Exit: C01–C03, C05–C10, C12–C14, and C16–C17 have focused checked-fact/diagnostic
tests. Existing ordinary-query snapshots remain unchanged. Keep the temporary
unsupported-feature gate until executable semantics and metadata consumers are
ready together.

### 3. Window membership, cardinality, and executable SQL

- Render a correlated candidate subquery per operand containing its internal row
  identity and ordinal. Reuse existing policy, optional-predicate, parameter, and
  order/limit/offset rendering. Append identity order tie-breakers before slicing.
  Each window is a complete-selection predicate boundary: pruning its whole
  `where` leaves that candidate unfiltered, not absent from composition.
- Implement intersection as left candidates filtered by membership in the right;
  implement union as left candidates plus right-only candidates. Preserve the
  identity tuple and deterministic composed ordinal through nested operators.
  Semi/anti joins or equivalent `EXISTS` checks avoid duplicate multiplication.
  A `UNION ALL` is valid only for the internally disjoint left/right-only identity
  branches, never for independently projected fragment JSON objects.
- Join the final identity set back to the source once and render the merged
  projection/child selections. Carry all correlation keys for nested parents.
  Each child composition is evaluated for every returned parent, independently
  of which parent operand matched, as required by the per-path contract.
- Order the final JSON aggregation explicitly by the composed ordinal; subquery
  ordering alone is insufficient. Keep identity columns and ordinals private,
  including when projected key fields are masked. Use one statement snapshot.
  Compare ordered results from independently generated cold/warm plans against
  unchanged seeded rows; repeat after whitespace and source-location changes.
- Add membership-expression bounds to shared cardinality logic. Intersection
  inherits an at-most-one proof from either side; union needs a shared single-row
  identity/catalog proof rather than two unrelated `limit 1` proofs. Final plan
  shape drives JSON envelopes and nullability.
- Apply existing generator limits to each operand window. Do not add a final
  truncation. Delay predicate-only optimizations until they prove preservation
  of both effective windows and the left-priority order; generator options can
  turn a source without an authored limit into a bounded candidate.
- Update formatted/compact SQL through the same renderer. If a predicate-fusion
  optimization is added, retain a complete-selection predicate wrapper so the
  existing `Optional`/`Absent` walker cannot erase an unfiltered OR operand.
- Keep the unsupported-feature gate through this batch. Activate only together
  with batch 4 once failed groups block artifacts, both operators execute
  correctly, and metadata consumers cannot mistake composed shapes for ordinary
  fragment reuse. Core/backend tests can inspect the candidate plan and renderer
  without publishing a production artifact during this staging period.

Exit: all semantic C01–C13 and C16–C17 cases have SQL snapshots and representative
live PostgreSQL result assertions. Include a row matching both OR branches exactly
once, SQL NULL, optional values on either side, missing versus unfiltered,
unique-key OR becoming plural, nested per-path behavior, and enforced row/field
policies. Include differing limits/offsets/directions, ties, keyless and composite-key
sources, overlapping identities with different requested fields, empty windows,
per-operand caps, and runtime-null windows. Verify both standard parameters and
operator/direction variants. Fix any exposed existing SQL-template collision
separately rather than hiding it in
composition normalization.

### 4. Metadata and TypeScript contracts

- Add required composition records and source-map entries to operation and fragment
  artifacts as specified. Serialize stable source/expansion occurrence IDs rather
  than Bowl identities; retain repeated operand provenance.
- Bump the manifest version from 7 to 8 or the next available version, regenerate
  metadata TypeScript/JSON Schema, and update all producers, readers, fixtures,
  and strict rejection tests in the same coherent batch. No dependency update or
  lockfile change should be needed.
- Disable standalone fragment result substitution for composition-bearing
  artifacts, including transitively inherited composition. Render the actual
  checked result fields and preserve scalar host/wire codecs and sparse result
  materialization. Keep ordinary artifacts' existing reuse behavior.
- Verify the daemon snapshot exposes the new provenance and one merged operation
  result. Runtime materializers consume normal compiled SQL/parameters, with no
  fragment-composition interpreter or branch-dependent client result shape.
- Remove the temporary unsupported-feature diagnostic in the coordinated
  activation commit covering completed SQL and versioned metadata consumers.

Exit: C07, C11, and C15 pass through generation and TypeScript consumer typechecks.
Include a standalone fragment with singular nested output whose composed use
becomes a collection, and a composition inherited through an ordinary spread.
Older manifests fail clearly in flat-manifest and daemon consumption paths.

### 5. Editor and adapter integration

- Complete and navigate individual operands using existing fragment resolution.
  Offer `and`/`or` only where legal, and keep binding-list completion separate
  from composition grouping. Show conflicts at the group with both source sites.
- Explain group operators, composed paths, and accepted window ownership from
  checked facts in hover; do not reconstruct composition in the LSP adapter.
- Extend semantic tokens, malformed-source completion sweeps, formatting, and
  embedded-host offset tests. Ensure comments and grouping survive formatting.
- Add daemon edit tests changing only `and` to `or`, grouping, bindings, and a
  cross-file provider. Failed edits retire artifacts and recovery republishes
  the correct versioned contract. Cover metadata-only provenance changes too:
  moved source ranges may change artifacts, but not the composed row order.

Exit: C14–C15 pass at core service and LSP/daemon protocol layers, including
UTF-16/host projection and cold-versus-warm artifact equivalence.

### 6. Conformance and completion gate

- Give C01–C17 named integration cases using neighboring `insta` conventions.
  Add a compact seeded Observatory composition example with independent
  contributors, shared/independent inputs, overlap, and policy context.
- Pin unrelated-root invocation isolation, final composed selection counts,
  and retirement after deleting the last operand/group. Require observed systems
  to exist before asserting zero-work deltas.
- Run targeted modules while iterating, then the complete required gates below.
  Inspect and accept snapshots with `cargo insta review`; never hand-edit them.
- Update the feature status in its spec, README, architecture note, this plan,
  and tracking issue only after all supported paths and rejection cases pass.

Exit: a three-fragment application example compiles to one truthful result shape
and SQL selection tree, requires no edits to shared fragments merely to remove
compatible duplicates, and reports every unsupported merge explicitly.

## Verification Commands

Use debug builds. During implementation, target the affected modules, for example:

```sh
cargo test -p dsql-core --test it composition
cargo test -p dsql-core --test it variables
cargo test -p dsql-core --test it sql
cargo test -p dsql-generate --test it
cargo test -p dsql-lsp --test protocol
cargo test -p dsql-daemon --test protocol
```

The `composition` module/filter is to be added; it is not a current passing gate.
Before completion:

```sh
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets
cargo check -p dsql-generate --no-default-features --target wasm32-unknown-unknown --lib
cargo test --workspace --all-targets
```

Run the database tests with a seeded disposable Observatory database and
`DSQL_OBSERVATORY_DATABASE_URL` set. Report missing prerequisites explicitly;
early-return database tests do not count as a live acceptance run.

From `integrations/typescript`: `bun run typecheck`, `bun run generate` followed
by a clean generated-artifact diff, and `bun run test:daemon`.
From `integrations/editor/tree-sitter`: `bun run check:surface`,
`bun run check:captures`, `bun run test:corpus`, `bun run test:fixtures`, and
`bun run test:indents` when indentation coverage changes.

## Risks And Design Review

The main risks are independent window membership/ordering, hidden identity
projection, optional-predicate boundaries, cardinality changes leaking
into reused fragment types, policy compatibility, and accidental input namespace
changes from new syntax ancestry. They have dedicated conformance cases and
must be settled in the shared compiler representation before adapter work.

The existing [expansion-growth issue](../../issues/dab86061-bound-fragment-expansion-occurrence-growth.md)
remains applicable. Do not multiply paths by distributing the operator tree or
duplicate full fragment bodies in the new representation. Record occurrence and
allocation growth while implementing; deterministic resource budgets remain a
separate tracked prerequisite for handling arbitrary untrusted projects at scale.

Window-preserving composition can require several candidate scans per relation
and parent. Measure representative two-/three-operand plans and database work,
including offset windows and heavily overlapping pages. Start with correct
membership subqueries; optimize shared windows only with semantic and ordering
equivalence evidence. No one-scan or large-project performance claim is made by
this plan.

Using the software-design skill's rubric, the current composition design is
approximately **7.5/10 as a plan**, not a verified implementation. The weakest
rows are implementation changes staying local and newcomers understanding the
boundary without reading multiple passes. The shared checked-result owner,
explicit no-contribution/window boundary types, and C01–C17 are the concrete
actions intended to close those gaps. Verify those properties against the code
after implementation; a diagram alone does not establish them.
