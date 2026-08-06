# Compiler architecture

dsql is built on [porridge](https://github.com/Nohac/porridge) (`bowl`): an
ECS-inspired incremental evaluation engine. The bowl *is* the compiler
database — there is no separate query layer, frontend crate, or side cache.
Facts (components) live on entities; compiler stages are systems that derive
more facts; adapters scoop the facts they need.

Read `docs/plan.md` for the port history and phase log; this document is the
durable description of how the compiler works.

## Source model

One text component, two writers (`dsql-core/src/source.rs`):

- `FilePath` — path identity, fingerprinted.
- `SourceText { rope }` — rope-backed text whose fingerprint is a hash of
  the rope's content. Identical re-loads and A→B→A edit sequences converge
  to no-ops, and derived embedded regions with unchanged text keep their
  revision (the equality cutoff extraction relies on). The earlier
  monotonic-revision design was replaced when embedding landed; a cheap
  revision fingerprint for editor roots (hashing a multi-megabyte rope per
  keystroke is wasted work) is part of the tracked source-residency design
  in `docs/issues.md`.
- **Analysis path**: project configuration assigns each physical file a
  resolver. The built-in `dsql` resolver marks the whole source as a
  `DsqlDocument`; named embedding resolvers mark an `EmbeddingHost` with an
  `ExtractionResolver`. Paths and extensions never choose the marker.
- **LSP path**: the same entity plus an `OpenBuffer` marker; `didChange`
  applies incremental rope edits through porridge external mutation
  (`Mut<SourceText>` + `apply_edit`).

Embedding extraction is provider-driven (`dsql-core/src/embedding.rs`). A
fingerprinted `ExtractionRegistry` maps configured resolver names to strategies;
the current `Regex` strategy derives region entities with content and callsite
ranges. Hosts carry only their resolver name, so adding a tree-sitter strategy
does not change project discovery, scope ownership, daemon reconciliation, or
LSP source admission. Multiple named providers may coexist in one bowl.

Spans are byte ranges end to end. Line/column (UTF-16) conversion happens
only at the LSP boundary (`dsql-lsp/src/position.rs`). Text materializes to
a contiguous `String` only at the parse boundary.

## Grammar: one source of truth

`dsql-core/src/grammar/dsql.llw` is the single definition of the language's
syntax. The vendored lelwel generates both:

- the parser and lossless CST (`Rule` enum, `CstData`, error recovery with
  structured `expected_tokens()` — a local patch, see `/vendor/PATCHES.md`);
- the logos lexer (`build_lexer`, a local patch): literal tokens become
  `#[token]` variants and `Token::literal_text()`; only the five non-literal
  tokens carry regex patterns, supplied by `build.rs`.

Nothing hand-mirrors the grammar. Adding a rule to `dsql.llw` fails
compilation until an entity claims it (below).

## Language entities

One vertical slice per language concept under `dsql-core/src/entities/`:
the concept's fact components, CST lowering, checks, and service
contributions live in one file. The stage traits (`dsql-core/src/entity.rs`)
form a compile-time coverage contract:

| Trait | Stage |
|---|---|
| `LanguageEntity` | identity + system registration |
| `LowerStage` | CST node → fact components |
| `FormatStage` | canonical text of owned rules |
| (retired) | hover/completion systems register in `LanguageEntity::register` |

`register_entity::<E>` bounds the syntax stages only. Rule ownership lives
in two exhaustive `match`es in `entities/lowering.rs` (`lower_rule`,
`format_rule`): every grammar rule is either claimed by an entity or
explicitly listed as structural. Semantic and service systems register in
`LanguageEntity::register`; a stage trait is only worth adding when it can
participate in exhaustive rule ownership.

The entities: `Document` (files, parse), `Definition` (queries and
fragments — one concept, `DefKind` branches), `FieldSelection`,
`FragmentSpread`, `Clause`, `Directive`, `Expression` (no facts: the typed
`Expr` tree is a plain value carried inside clause/directive facts),
`Variable` (per-occurrence facts for set-oriented inference).

## The fact tree

Lowering (`lower_syntax_facts`) walks the CST once per file and dispatches each
rule node to its owner. Facts carry:

- `NodeKey { file, node }` — stable identity within one parse;
- `ChildOf` — relationship edge to the nearest enclosing
  selection/definition; the engine maintains the `Children` inverse
  (sibling *source* order is span order — the inverse is entity-ordered);
- `SemanticMemberOf` — untracked relationship plumbing from every nested
  syntax fact to a dedicated group for its enclosing query, fragment, filter,
  or condition. Lowering is its only writer. The engine-maintained,
  fingerprinted `SemanticMembers` inverse lives on the group, not the semantic
  root, so membership changes cannot invalidate facts anchored to the root;
- `SemanticRoot` — the root entity referenced by a semantic group. The group
  and root share a `NodeKey`: relationship-scoped consumers drive from the
  group and bind the root second through that key. This direction lets a
  member change translate to its exact driving group in one scheduler hint;
- `BelongsToFile` — join key for per-file filtering;
- `DerivedFrom` — ownership: any text change re-lowers the file and retires
  every fact derived from it.

`SemanticMembers` records relationship membership, while
`Related<SemanticMembers, P>` returns only members satisfying the required
parts in `P`. A missing required part therefore removes a row from the fetched
set but remains an observed dependency: adding it reruns that one group.
Semantic roots never carry `SemanticMemberOf`, including if the grammar later
permits nested definitions.

Definition and spread facts also carry their file's `ResolutionScope`
(docs/spec/resolution-scopes.md): fragments resolve against the *effective
resolver* of the spread's scope — the scope's own fragments plus its direct
imports', configured per project as the fingerprinted `ScopeImports`
singleton. Exactly one visible candidate resolves; duplicates (per scope),
local-vs-import collisions, and import ambiguities are diagnostics. Pure
bound joins cannot express "local or imported", so spread resolution is an
index-tracked per-spread system: the fingerprinted `DefIndex` and
`ScopeImports` are tracked inputs, so rows rerun exactly when the
definition set or the scope graph changes.

Field and clause diagnostics consume exact resolution rows at Evaluate. A
`ResolvedSelection` carries structured lookup failures and the owned scalar
type contract needed by its field check. A `ResolvedClause` carries the same
owned type contracts for paths and order items plus its syntax `NodeKey`, so
each resolution context binds its one source clause without an ambient view or
catalog re-read. Catalog-wide resolution still observes the catalog singleton;
fingerprint cutoff on unchanged resolution rows prevents unrelated checks from
rerunning.

The remaining Complete-phase definition check is deliberately residual: it
still walks selection syntax for output-key expansion, fragment compatibility
and cycles, and operation-wide policy/table summaries. Those set-wide results
will become bottom-up relationship-owned summary facts before the walk is
removed; field and clause diagnostics no longer depend on it.

## Stages and demand

Checks, inference, planning, and SQL generation are systems gated on demand
markers — no demand, no work planned:

| Marker | Stage |
|---|---|
| `DiagnosticsDemand` | catalog checks, duplicate/unknown-fragment checks |
| `VariablesDemand` | variable-binding inference (`VariableBinding` facts) |
| `PlanDemand` | query planning (`QueryPlanFact`) |
| `SqlDemand` | PostgreSQL rendering (`GeneratedSqlFact`, `SqlOptions`) |

Diagnostics are entities carrying the full component set (`DiagnosticFacts`:
span, severity, source stage, machine-readable `DiagnosticCode`, message),
anchored by `DerivedFrom` to the facts (and catalog) that produced them —
they retire automatically when either changes.

The catalog is a fingerprinted singleton (`CatalogSnapshot`); replacing it
reruns exactly the checks that track it. Set-reactive checks additionally
track the fingerprinted `DefIndex` so a `View` over other definitions cannot
go stale.

### Catalog ownership and the effective boundary

PostgreSQL introspection produces the **generated catalog** as replaceable YAML
under the configured schema directory. Authored changes do not belong in those
files. `dsql-project` loads that provider output, composes the authored YAML
under `dsql/overlays/`, validates the resulting graph, and constructs the
single `CatalogSnapshot` inserted into the bowl.

The snapshot is the compiler's **effective catalog** boundary: resolution,
checks, policies, planning, SQL, services, and generation never consult catalog
sources independently. [Catalog overlays](../spec/catalog-overlays.md) retain
provider and authored proof provenance at this project boundary, keeping
composition and conflict reporting out of language systems.

### Effective input contracts

Variable inference publishes one contract entity per query or fragment. Its
`DefinitionVariables` component contains the effective public bindings, while
the private `DefinitionInputRewrites` component contains the validated input
rewrite for every reachable fragment-spread entity. Keeping the entity-keyed
planner state separate lets hover, completion, and metadata track stable
bindings without rerunning for rewrite-only changes. Local occurrences remain
separate `VariableBinding` facts so hover and occurrence diagnostics retain
their source spans.

The contract evaluator infers each definition's local inputs once and memoizes
completed recursive fragment contracts for the duration of that evaluation.
Cycles return an empty cut result without entering the memo, so a path that
first encounters a definition through a cycle cannot poison a later acyclic
use. One spread-root decision (`Contained`, whole-root lift, or explicit leaf
bindings) produces both the caller-facing bindings and the planner rewrite;
invalid decisions therefore cannot disagree between inference and SQL
planning. Trusted `context` inputs pass through spreads unchanged and merge by
the same compatibility rules as public inputs.

Definition headers refine the effective bindings, not individual occurrence
facts. A refinement may make an allowed binding nullable and may attach a typed
default; omission, explicit `null`, and structural absence remain distinct in
metadata and execution. Fragment spreads apply one of four checked shapes:
containment, whole-root lifting, namespaced root lifting, or explicit leaf
bindings. The same spread decision produces caller paths and planner rewrites,
so code generation, runtime materialization, and SQL cannot disagree about a
fragment's public contract.

Planning is driven by the published contract fact rather than re-walking
fragment inputs. The fact also carries a private `DefinitionVariableOwner`
snapshot containing the source definition, declaration, and resolution scope.
Keeping that owner data behind a distinct component avoids making the derived
contract entity look like another lowered definition to ambient tree views,
while making the contract a tracked planning input. Per-spread rewrite maps use
the target fragment's local coordinates and compose as the planner enters
nested spreads.

Planning emits one `QueryPlanFact` per query definition. Its ordered root plans
share one operation-level policy-context contract and one fragment-spread
provenance set. SQL generation consumes that complete definition plan through a
single template context, so parameters and dynamic variants deduplicate across
roots. A multi-root definition renders each root as a one-row subquery and
cross-joins those subqueries into one result row; generation consequently
publishes one operation artifact under the source definition name.

### Policies and filters

Filters and reusable conditions lower as standalone `PolicyDecl` facts and use
the same resolution scopes as fragments. `PolicyIndex` fingerprints names,
visibility, and concrete or structural catalog matches. `PolicyBodyIndex`
fingerprints rule bodies separately, so editing a predicate recompiles policy
semantics without invalidating consumers that only need the match set.

Policy compilation resolves conditions, row predicates, field guards, trusted
context requirements, and enforcement into `CompiledPolicyIndex`.
`PolicyPlanIndex` colocates that body-sensitive result with the definition index
used by planning. Query filter assignments are resolved once against these
tracked facts; checks, planning, SQL, metadata, hover, completion,
go-to-definition, semantic tokens, and lock generation consume the resulting
policy identities and matches rather than interpreting policy source again.

## Services

Request/response through bound entities: insert `(HoverRequest, FilePath,
Position)`, `bind().take::<HoverInfo>()`. Enrichment is an *outer* join on
the file path (one invocation per match, one `None` invocation otherwise),
so a single system seeds the answer scaffold for resolved and unresolved
requests alike. Entity candidate systems insert candidates addressed by a
`RequestKey` — fully tracked ones run phase-free, and only those still
reading lowered facts ambiently sit behind the Complete barrier; arbitration consumes them *tracked* — one invocation per
(request, candidate) pair, upgrading the answer in place through `MutRef`
as a commutative fold (max for hover, sorted set-union for completion).
Nothing answers at Settle: settle-phase inserts defer to the next run, and
the engine's entity-granular same-phase race flag enforces the
ambient-vs-tracked discipline throughout.

## Crates

- `dsql-core` — everything above.
- `dsql-project` — `dsql.toml`, resolution scopes, source discovery,
  generated-catalog loading, and `open_project_bowl`.
- `dsql-metadata` — the stable serialized artifact and manifest contracts.
- `dsql-generate` — settled-fact assembly and transactional artifact
  publication.
- `dsql-introspection` — PostgreSQL catalog introspection.
- `dsql-execute` — strict metadata-driven PostgreSQL operation execution.
- `dsql-daemon` — the resident FIFO build protocol and reconciliation loop.
- `dsql-cli` — project commands, generation, daemon entrypoint, metadata
  schemas, and operation listing/execution.
- `dsql-lsp` — tower-lsp-server adapter: live buffers, published
  diagnostics, hover, definition, formatting.
- `integrations/typescript` — daemon client, Vite rewriting, browser-safe
  operation objects, server execution payloads, and renderer hooks.
- `vendor/lelwel`, `vendor/logos` — vendored via git subtree; every local
  change is ledgered in `/vendor/PATCHES.md`.

## Testing

Integration over unit: each crate has one `tests/it` harness; insta
snapshots pin CSTs, settled facts, diagnostics, variable bindings, plans,
generated SQL, formatter output, and service answers. Snapshot only settled
bowl state, sorted for stability. The imdb schema fixtures drive
catalog-dependent tests.
