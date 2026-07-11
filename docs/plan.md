# DSQL on Porridge — Architecture and Port Plan

This repository is a rewrite of `dsql-poc` with [porridge](https://github.com/Nohac/porridge)
as the incremental evaluation engine. The language core is rebuilt as **language
entities** (the porridge playground pattern) that carry over the **language atom
principles** from `dsql-poc/docs/proposals/language-atoms.md`.

Status: **historical port record** (updated 2026-07-11). The port completed
through phase 12; this document preserves the plan and its decision log.
Current invariants live in `docs/architecture/compiler.md`, current gaps in
`docs/issues.md` and `docs/codebase-review.md`.

## Goals

1. **Porridge-native architecture.** The bowl is the compiler database: source
   files, CSTs, language facts, diagnostics, plans, and SQL are all components
   on entities. Compiler stages are systems. Incrementality, memoization, and
   demand-driven evaluation come from porridge, not from a bespoke query layer
   (dsql-poc used picante for this; it is replaced entirely).
2. **Atom principles, entity shape.** Keep the guarantees of the language-atoms
   proposal (single rule ownership, compile-time stage coverage, explicit
   no-effect declarations) but implement them the playground way: plain traits,
   one exhaustive `match` on the generated `Rule` enum, no registry or macro
   machinery unless boilerplate forces it.
3. **DRY: generated code is the source of truth.** The lelwel-generated parser
   (`Rule` enum, CST) is the canonical definition of language constructs.
   Nothing hand-mirrors it. Where dsql-poc duplicated grammar knowledge
   (see "Known duplication" below), this repo generates or statically checks it.
4. **Same general layout as dsql-poc**, trimmed to what each phase needs.

## What we take from where

| From | What |
|------|------|
| dsql-poc | The grammar (`dsql.llw`), the language itself (syntax, directives, variables, clauses), atom principles, stage list, catalog/plan/SQL design, formatter approach, insta-heavy integration testing, fixture queries |
| porridge playground | Entity contract (`LanguageEntity` + stage traits + `register_entity`), exhaustive `lower_rule` dispatch, `LowerCtx` walk, candidate-fact service pipeline, demand markers, fingerprinted index singletons, bound joins, `DerivedFrom` diagnostics |
| porridge (`bowl`) | The engine: components, entities, systems, queries, phases, hooks |

## Dependencies

- `bowl` (and `macros` transitively) as a **git dependency** on
  `https://github.com/Nohac/porridge`, pinned to a `rev`. Bump deliberately.
- `lelwel` and `logos` **vendored in-repo** (see below). dsql-poc already
  depends on a locally patched lelwel checkout (`record_expected_tokens`, used
  for error recovery and completions), so the patch must live in this repo to
  be reproducible. Vendoring both unblocks the lexer-generation work below and
  any logos changes it turns out to need. They may move out to standalone
  forks/crates later; the vendoring discipline is designed to keep that cheap.
- Rust edition 2024 (matches porridge).

### Vendoring: `git subtree`, not submodules

Both dependencies are vendored with `git subtree --squash` into `vendor/`:

```
git subtree add --prefix vendor/lelwel https://github.com/0x2a-42/lelwel.git v0.10.4 --squash
git subtree add --prefix vendor/logos  https://github.com/maciejhirsz/logos.git v0.16.0 --squash
```

Why subtree over submodules: local patches become ordinary commits in *this*
repo (no fork hosting required, no detached-HEAD bookkeeping), a vendor change
and the consumer change that needs it land in **one atomic commit**, fresh
clones and CI need no `--recursive` step, and `git subtree pull` can still
merge upstream releases later. Squashing keeps upstream history out of our log.

Discipline (also in CONTRIBUTING.md):

- `vendor/PATCHES.md` documents, per crate: upstream URL, the vendored
  tag/rev, and **every local change** — what, where, and why. This file is the
  DRY ledger; if a change is not listed there, it does not exist.
- Each functional local change is its own commit, subject-prefixed
  `vendor(lelwel):` / `vendor(logos):`, and updates `PATCHES.md` in the same
  commit.
- Keep changes rebasable: prefer additive hooks (new methods, new emit passes)
  over rewrites of upstream code, so `git subtree pull` merges stay tractable
  and upstreaming remains possible.
- `logos` is a crate family (`logos`, `logos-codegen`, `logos-derive`); the
  subtree carries the whole workspace and our crates take `path` deps into it.

## Architecture

### The bowl as the compiler database

Everything is a component; there is no side database.

- A **source file** is an entity with `FilePath` + `FileText`.
- A **parse system** derives `ParsedFile` (CST) plus parse `Diagnostic`
  entities (`DerivedFrom` the file, so they clean up when the file changes).
- One generic **lowering walk** (`lower_syntax_facts` in playground terms) visits the
  CST once per file and hands each rule node to the entity that owns it. Owners
  emit **fact components** (typed, span-carrying) as new entities keyed to the
  file.
- **Check / lint / variable-inference / plan / SQL systems** are registered by
  the entities that own the constructs, gated on demand markers
  (`DiagnosticsDemand`, `PlanDemand`, `SqlDemand`, ...), and emit further facts
  or diagnostics.
- **Indexes** (fragment index, definition index, catalog snapshots) are
  fingerprinted singleton components (`#[component(hash)]`), so an unchanged
  set invalidates nothing.
- **Cross-construct resolution** (fragment spread → fragment def, path → catalog
  relation) uses bound joins (`Query<..., Where<Eq<Key>>>`) instead of manual
  lookups.
- **Services** (hover, completion, goto-definition, semantic tokens) use the
  candidate-fact pipeline: enrichment (Phase::Complete) → per-entity candidate
  systems → finalizer (Phase::Cleanup) picks by priority. External callers use
  request entities + `bind().take::<Response>()`.

### Source model: batch analysis vs live LSP buffers

dsql-poc spent real design effort separating the two ways text enters the
compiler: **analysis** just needs to open a file once and emit facts, while the
**LSP** holds a document open and mutates it rapidly (ropey). That split is
kept, but in porridge terms the difference collapses to *who writes the text
component* — everything downstream is identical:

- One text component, rope-backed:

  ```rust
  /// Source text of a file entity. Fingerprint is a revision counter bumped
  /// on every edit, so unchanged text invalidates nothing and fingerprinting
  /// never rehashes the rope.
  #[derive(Component)]
  #[component(hash)]        // fingerprint = revision
  pub struct SourceText { rope: Rope, revision: u64 }
  ```

- **Analysis path (CLI, generate, tests):** a loader reads the file from disk
  and inserts an entity with `FilePath` + `SourceText`, settle, read facts.
  The file is never "held open"; the component simply is the fact.
- **LSP path:** `didOpen` inserts the same components plus an `OpenBuffer`
  marker (LSP owns the text; disk watchers must not overwrite it). `didChange`
  applies incremental edits to the rope in place via porridge's external
  mutation (`Mut<SourceText>` / `Cow<SourceText>`), bumping `revision`.
  Porridge epochs freeze inputs per generation, so rapid edit bursts are safe
  while a settle is in flight; the next generation sees the latest snapshot.
  `didClose` removes `OpenBuffer` and reverts to disk state.
- **Debounce is data, not timers:** diagnostics and other expensive stages are
  gated on demand markers; the LSP inserts `DiagnosticsDemand` when the editor
  goes idle, not on every keystroke.
- **Rope discipline (carried over from dsql-poc):** spans are byte ranges
  end-to-end; line/column (UTF-16) conversion happens only at the protocol
  boundary using the rope's line index; full-text materialization happens only
  where an API demands contiguous `&str` — today that is the parse boundary
  (logos lexes `&str` only), which needs the whole text anyway.

The parse system, lowering walk, checks, and services never know which path
produced the text.

### Language entities = language atoms

One file per language concept under `crates/dsql-core/src/entities/`. Each file
co-locates the concept's fact components, CST lowering, checks, and service
contributions — the "vertical slice" both projects converged on.

The stage traits cover *syntax* only (updated 2026-07-11): `LowerStage` and
`FormatStage` join the exhaustive generated-rule dispatch, so a new
construct cannot compile without them. Everything else — checks, lints,
variables, planning, services — registers as ordinary systems inside
`LanguageEntity::register`; the bowl schema and declared system outputs are
the coverage contract for behavior. The per-stage trait matrix below is the
original atom-era plan, kept for the record:

| Atom stage (dsql-poc) | Entity trait here | Coverage |
|---|---|---|
| build_ast + lower | `LowerStage` | required (merged — see decision 1) |
| format | `FormatStage` | required |
| check | plain system registration | retired as a trait |
| lint | plain system registration | retired as a trait |

`register_entity::<E>` bounds on every stage trait, so a new entity fails to
compile until it declares each stage. A stage that does not apply is an
explicit empty/no-effect impl with a doc comment stating why (the atom
proposal's `no_effect("...")` rationale becomes the doc comment). If the
no-effect boilerplate gets heavy across ~12 entities × ~9 stages, add a small
declarative macro then — not before.

Rule ownership lives in one exhaustive `match rule { ... }` in
`entities/mod.rs`. Structural rules (consumed by their owning ancestors) are
explicitly listed with a comment. Adding a rule to `dsql.llw` fails compilation
until it is claimed. This gives the atom proposal's guarantees — single
ownership, no orphan rules, drift breaks the build — with zero registry code.

### Entity inventory and rule ownership

Grammar rules from `dsql.llw` → owning entity (initial cut; the exhaustive
match is the real source of truth once implemented):

| Entity | Owns rules | Notes |
|---|---|---|
| `Document` | `document` | file root; parse system, parse diagnostics |
| `Definition` | `query_def`, `fragment_def` | one entity: queries and fragments are the same concept (named def + selection set) everywhere except where `DefKind` branches; fingerprinted `DefIndex`, duplicate-fragment check |
| `FieldSelection` | `field_selection`, `field_selection_tail`, `field_suffix` | selection facts (flat `NodeKey`/`ParentKey` tree encoding), catalog field checks, relation selectors |
| `FragmentSpread` | `fragment_spread` | spread facts; resolution via bound join on `FragmentKey` + `BelongsToFile` |
| `Clause` | `where_clause`, `order_by_clause`, `limit_clause`, `offset_clause` (consumes `clause_list`, `clause`, `order_item`, `sort_direction`) | one entity, `ClauseFact` enum carrying typed expression trees |
| `Directive` | `directive` (consumes `directive_name`, `directive_namespace`, `directive_member`, `directive_argument`) | directive registry checks |
| `Expression` | `expr`, `binary_expr`, `literal`, `binary_operator`, `comparison_operator`, `scoped_path`, `scoped_path_segment` | no facts of its own: the typed `Expr` tree is a plain value built by `build_expr` and carried inside clause/directive facts (paths folded in here) |
| `Variable` | `value_variable`, `operator_variable` | per-occurrence `VariableUse` facts for set-oriented inference, in addition to their structural place in `Expr` trees |
| structural | `definition`, `selection`, `selection_set`, `qualified_name`, `relation_ref` | elided/consumed by owners |

### Pipeline phases

- **Startup**: load catalog (schema snapshot components; later: live
  introspection), directive registry, project config.
- **Evaluate**: parse, lowering walk, per-entity checks (gated on
  `DiagnosticsDemand`), variable inference.
- **Complete**: index aggregation (fingerprinted singletons), planning (gated
  on `PlanDemand`), SQL generation (gated on `SqlDemand`), service enrichment +
  candidates.
- **Cleanup**: service finalizers, `cleanup_stale_derived`.

### DRY: known duplication in dsql-poc and how this repo removes it

1. **`SyntaxRule` hand-mirror of the generated `Rule` enum** — gone. All
   dispatch, editor context classification, and formatting key directly off
   the generated `Rule`.
2. **Token strings defined twice** (`.llw` token declarations *and* logos
   `#[token]` attributes on a hand-written `Token` enum) — *done (phase 8)*:
   the vendored lelwel gained `build_lexer(path, &LexerSpec)`, which emits
   the whole logos lexer from the grammar's token section. Literal tokens
   (`Query='query'`, `Eq='=='`) become `#[token]` variants and
   `Token::literal_text()`; only the five non-literal tokens (`Name`,
   `String`, `Number`, `Whitespace`, `Comment`) carry regex patterns,
   supplied by `build.rs`. A token with neither fails the build. This also
   replaced dsql-poc's hand-listed `completion_label()` keyword table:
   editor layers select from `literal_text()` instead.
3. **Loose sibling-path lelwel dep** — replaced by in-repo `vendor/lelwel/`
   carrying the existing `record_expected_tokens` patch.

### Workspace layout

Mirrors dsql-poc, created as needed per phase:

```
Cargo.toml              # workspace
CLAUDE.md               # agent rules (commits, code style)
CONTRIBUTING.md         # how to run tools, testing conventions
docs/                   # this plan, architecture notes, spec (ported/adapted from dsql-poc)
vendor/
  PATCHES.md            # the vendor change ledger: upstream revs + every local change
  lelwel/               # vendored parser generator (subtree)
  logos/                # vendored lexer generator (subtree, whole crate family)
crates/
  dsql-core/            # the language: grammar, entities, facts, stages, services
    build.rs            # lelwel::build("src/grammar/dsql.llw")
    src/
      grammar/          # dsql.llw, lexer (generated in phase 8), generated parser
      entity.rs         # stage-trait contract + register_entity
      source.rs         # SourceText (rope), FilePath, OpenBuffer, loaders
      entities/         # one file per language concept (vertical slices)
      facts.rs          # cross-cutting components (Span, Diagnostic, demand markers)
      catalog/          # schema catalog components + loading
      service/          # candidate-fact pipelines (hover, completion, ...)
      format/           # CST-based conservative formatter
      sql/              # SQL rendering (sea-query)
    tests/it/           # integration tests (insta snapshots) — single `it` harness
  dsql-cli/             # phase 10+
  dsql-lsp/             # phase 10+
  dsql-project/         # phase 10+ (config, file discovery)
  dsql-metadata/        # phase 10+ (serializable artifact schemas)
  dsql-generate/        # later
  dsql-introspection/   # later
  dsql-embedding/       # later
```

Copy from dsql-poc where cheaper than rewriting: `dsql.llw`, the logos lexer,
fixture queries (`tests/queries/valid|invalid`), schema fixtures, spec docs.

### Testing

- Integration tests over unit tests; one `tests/it/` harness per crate with
  modules per area (parse, facts, diagnostics, variables, plan, sql, format,
  services) — uv's `it` layout, dsql-poc's module split.
- insta snapshots for everything user-visible: CST debug output, settled facts
  (stable ordering!), diagnostics, plans, generated SQL, formatter output,
  service responses.
- Fixture `.dsql` files ported from dsql-poc; invalid fixtures drive
  diagnostic snapshots.
- Bowl-level determinism: snapshot only settled state, never mid-settle.

## Phases

Each phase lands as one or more atomic commits with tests.

0. **Scaffolding** — workspace, docs, CLAUDE.md, CONTRIBUTING.md. *(this change)*
1. **Grammar + parse** — vendor lelwel + logos as subtrees, apply the
   `record_expected_tokens` patch, start `vendor/PATCHES.md`; copy `dsql.llw`
   + lexer; `dsql-core` with build.rs; parse to CST; CST snapshot tests.
2. **Entity skeleton** — bowl dep; rope-backed `SourceText` + disk loader
   (analysis path); entity contract traits; exhaustive `lower_rule` dispatch
   (everything structural at first); `Document` entity: file entities, parse
   system, parse diagnostics via `DerivedFrom`; first settled-state snapshots.
3. **Definitions** — `QueryDef`, `FragmentDef` entities; fingerprinted def
   index; duplicate-name diagnostics (demand-gated).
4. **Selections** — `FieldSelection`, `FragmentSpread`; fragment resolution via
   bound join; unresolved-spread diagnostics.
5. **Leaf constructs** — `Clause`, `Expression`, `Path`, `Variable`,
   `Directive` entities; directive registry checks.
6. **Catalog** — schema snapshot components (ported fixture format); path and
   field checking against the catalog.
7. **Variables** — build-time vs query-time inference; operator variables;
   variable snapshots.
8. **Lexer DRY** — extend vendored lelwel to emit the logos lexer from
   `dsql.llw`; delete the hand-written lexer.
9. **Plan + SQL** — plan facts; sea-query PostgreSQL rendering; SQL snapshots.
10. **Formatting** — CST-based conservative formatter (trivia-preserving,
    refuses on parse errors).
11. **Services** — hover via candidate pipeline; then completion, definition,
    semantic tokens; then `dsql-lsp` (live-buffer path: `OpenBuffer`,
    incremental rope edits via external mutation, demand-marker debounce) +
    `dsql-project` + `dsql-cli` crates.
12. **The rest** — metadata, generate, introspection, embedding, TypeScript
    integration, ported on demand.

## Design decisions (with rationale)

1. **No separate AST layer.** dsql-poc has CST → AST → lowered → checked;
   the atom proposal gives each atom both an `Ast` and a `Lowered` type. Here
   entities lower the CST **directly into fact components** — the facts *are*
   the typed model. This deletes one full representation and its builder,
   fits the playground pattern exactly, and formatting never needed the AST
   (it is CST-based). Entities that want richer local structure keep private
   helper types inside their own file.
2. **Traits + exhaustive match instead of registries/macros.** The atom
   proposal's registry indirection existed so stages don't call atoms by name;
   the playground shows the same drift guarantees fall out of trait bounds and
   one `match`. Less machinery, better jump-to-definition, same compile-time
   coverage.
3. **One `Clause` entity, not four.** where/order/limit/offset share shape,
   checks, and planning surface; a kind enum keeps the exhaustive match small.
   Split later if one clause grows real independent behavior.
4. **Vendor lelwel from phase 1**, not when first patch is "needed" — the
   patch is already needed (dsql-poc's checkout is already dirty), and an
   unversioned sibling path dep is the reproducibility bug this repo fixes.
5. **Porridge stays a git dep, pinned.** It is actively evolving (streaming
   evaluation planned); pinning a rev keeps upgrades deliberate. Design only
   against documented patterns (demand markers, phases, bound joins,
   `DerivedFrom`), which are the stable surface per its spec/.
6. **Subtree vendoring with a patch ledger.** Submodules were rejected: they
   require hosting forks to carry patches, split a vendor change and its
   consumer change across two repos, and add clone/CI friction. Subtrees keep
   patches as atomic in-repo commits, `vendor/PATCHES.md` keeps them
   documented and extractable into standalone forks later.
7. **No frontend crate — the bowl is the frontend.** dsql-poc's frontend
   (source DB, picante orchestration, resolution contexts) dissolves into
   components and systems: `SourceText` is the source DB, memoized systems are
   the query layer, demand markers are the scheduler. Adapters (LSP, CLI)
   talk to the bowl directly.
8. **One source model, two writers.** Analysis loads a file from disk and
   inserts `SourceText` once; the LSP holds `OpenBuffer` entities and applies
   incremental rope edits via external mutation. Downstream systems cannot
   tell the difference — the analysis/LSP split from dsql-poc is preserved
   without a `SourceDb` abstraction.

## Open questions

- **Editor stage granularity**: one `EditorStage` trait vs one trait per
  service. RESOLVED 2026-07-11: per-service traits are retired — they were
  registration ceremony from the POC atom system and proved nothing; only
  `LowerStage`/`FormatStage` stay (exhaustive generated-rule ownership).
- **Lexer regex syntax in `.llw`**: extend lelwel's grammar syntax vs sidecar
  annotations. Decide in phase 8 with upstream (0x2a-42/lelwel) compatibility
  in mind — keep the vendored fork rebasable.
- **Edit-burst mutation primitive**: `Mut<SourceText>` vs `Cow<SourceText>`
  for LSP `didChange` under load (porridge's `Cow` semantics are still
  evolving — see its TODO §8). Decide in phase 11 against the porridge rev
  pinned at the time.
