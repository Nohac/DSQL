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
- `BelongsToFile` — join key for per-file filtering;
- `DerivedFrom` — ownership: any text change re-lowers the file and retires
  every fact derived from it.

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
- `dsql-project` — `dsql.toml` discovery/parsing, schema metadata loading,
  document discovery, `open_project_bowl`.
- `dsql-cli` — `dsql check | sql | fmt` over a project bowl.
- `dsql-lsp` — tower-lsp-server adapter: live buffers, published
  diagnostics, hover, definition, formatting.
- `vendor/lelwel`, `vendor/logos` — vendored via git subtree; every local
  change is ledgered in `/vendor/PATCHES.md`.

## Testing

Integration over unit: each crate has one `tests/it` harness; insta
snapshots pin CSTs, settled facts, diagnostics, variable bindings, plans,
generated SQL, formatter output, and service answers. Snapshot only settled
bowl state, sorted for stability. The imdb schema fixtures drive
catalog-dependent tests.
