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
- `SourceText { rope, revision }` — rope-backed text. The fingerprint is a
  process-global monotonic revision, bumped by every mutation, so edit
  bursts never rehash content and wholesale replacement can never collide.
- **Analysis path**: `load_file`/`insert_source` reads a file once; the
  component simply is the fact.
- **LSP path**: the same entity plus an `OpenBuffer` marker; `didChange`
  applies incremental rope edits through porridge external mutation
  (`Mut<SourceText>` + `apply_edit`).

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
| `HoverStage` | hover candidate systems (or explicit no-op) |

`register_entity::<E>` bounds on every stage, so a new entity does not
compile until it declares each one. Rule ownership lives in two exhaustive
`match`es in `entities/lowering.rs` (`lower_rule`, `format_rule`): every
grammar rule is either claimed by an entity or explicitly listed as
structural. Adding a stage trait retroactively forces every entity to
declare it.

The entities: `Document` (files, parse), `Definition` (queries and
fragments — one concept, `DefKind` branches), `FieldSelection`,
`FragmentSpread`, `Clause`, `Directive`, `Expression` (no facts: the typed
`Expr` tree is a plain value carried inside clause/directive facts),
`Variable` (per-occurrence facts for set-oriented inference).

## The fact tree

Lowering (`generate_ast`) walks the CST once per file and dispatches each
rule node to its owner. Facts carry:

- `NodeKey { file, node }` — stable identity within one parse;
- `ParentKey` — the nearest enclosing selection/definition, the flat
  encoding of the selection tree (sibling order is span order);
- `BelongsToFile` — join key for per-file filtering;
- `DerivedFrom` — ownership: any text change re-lowers the file and retires
  every fact derived from it.

Cross-construct joins are porridge bound joins: fragment-spread resolution
pairs `FragmentKey` + `BelongsToFile` on both sides, one invocation per
(spread, fragment) pair.

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
Position)`, `bind().take::<HoverInfo>()`. Enrichment (Phase::Complete)
resolves the request's file via a bound join; entity hover systems insert
`HoverCandidate` facts for spans containing the cursor; a finalizer
(Phase::Cleanup) picks the highest-priority candidate. Arbitration is data,
not call order. Go-to-definition follows `SpreadResolution` facts the same
way.

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
