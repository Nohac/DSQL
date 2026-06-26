# Compiler Architecture

This document describes the compiler and frontend analysis architecture that is
implemented today. Future architecture proposals live in separate files under
[`../proposals`](../proposals).
behavior lives under [`../spec`](../spec).

## Layering

DSQL keeps syntax, semantic analysis, project state, and adapters separate:

- `dsql-core` owns parsing, AST/CST types, lowering, definition extraction,
  checking, linting, planning, formatting, SQL generation, diagnostics, and
  catalog data structures.
- `dsql-frontend` owns project-local source state, resolution contexts,
  incremental query orchestration, LSP-style interactive analysis, and
  presentation of diagnostics back to physical documents.
- `dsql-lsp`, `dsql-cli`, `dsql-generate`, and TypeScript integration code are
  adapters over core/frontend APIs. They should not contain language rules.

The compiler core should remain pure and reusable. Mutable project and editor
state belongs at the frontend boundary.

## Source Model

Source text enters analysis through immutable document revisions owned by
`SourceDb`. A document revision is the canonical source object for one physical
document at one revision:

```rust
pub struct SourceDocumentRevision {
    pub id: PhysicalDocumentId,
    pub path: Option<PathBuf>,
    pub revision: RevisionId,
    pub rope: Rope,
    pub full_text: OnceLock<Arc<str>>,
    pub residency: SourceResidency,
}
```

`SourceDb` is the only owner of loaded project files and live editor buffers.
The Rope remains the editing and source-query representation. The optional
full-text cache records the contiguous document text only when a stage already
needs the full source, for example embedded-source extraction or a parser API
that requires `&str`.

Compiler source units are regions over document revisions, not detached source
copies:

```rust
pub struct SourceRegion {
    pub document: Arc<SourceDocumentRevision>,
    pub content_range: TextRange,
    pub source_offset: u32,
}

pub struct SourceSnapshot {
    region: SourceRegion,
}
```

A full `.dsql` file is represented as a region covering the entire document.
Embedded DSQL is represented as a region over the host document, with
`source_offset` equal to the embedded content start. Embedded regions should not
become independent canonical documents.

APIs that need contiguous source text call `SourceSnapshot::source_view()`. It
borrows from the document full-text cache when available, borrows directly from
the Rope when the requested region is contiguous, and otherwise materializes the
document text once for the current document revision. This keeps repeated
embedding, parsing, formatting, and diagnostic work from flattening the same
source multiple times.

Compiler stages should prefer Rope operations, ranges, or short borrowed token
text over full-source strings. Full-source materialization belongs only at true
output boundaries or temporary `&str`-based integration points. When
materialization is required, it should happen through the document revision so
all source regions for that revision can share it.

Spans are stored as byte ranges internally. Editor-facing line/character
positions are computed only at the protocol or presentation boundary.

`ProjectHost` owns a `SourceDb` for project and editor state. `SourceDb` stores
physical document revisions and tracks source-unit membership by
`ProjectSourceRegion`.

```rust
pub enum SourceResidency {
    AnalysisSnapshot,
    OpenEditable,
}
```

Open editor buffers use `OpenEditable` residency and are updated by Rope range
edits. Project-loaded files use `AnalysisSnapshot` residency.

`SourceUnitId` is the compiler/project source-unit identity. A physical
document can contain one full-file DSQL source unit or multiple embedded units,
for example DSQL regions extracted from a TypeScript file. `SourceDb` allocates
and reuses `SourceUnitId` values for stable `ProjectSourceRegion` keys.

`CompilerDb` publishes `SourceRegion` values as `SourceInput`, keyed by
`SourceUnitId` and revision. Tracked analysis can therefore depend on immutable
source regions while all loaded source text remains owned by `SourceDb`.

## Parsing And Syntax

The parser describes source structure. It should not perform catalog or
resolution work.

Current syntax architecture:

- generated Lelwel parser and Logos lexer APIs stay behind local wrappers;
- `parse_source` returns a `ParseResult` containing the original
  `SourceSnapshot`, syntax tree, AST-facing `SourceFile`, and parse
  diagnostics;
- the CST and source text remain the source of truth for rewriting and
  formatting;
- the AST gives typed access for compiler stages, not a way to reconstruct user
  text.

Qualified names and relation references are parsed structurally. For example,
schema/table and relation selector spelling are preserved as separate AST
fields instead of being collapsed into strings and split later.

Parser structure should be specific enough for downstream semantic and editor
features to avoid reparsing source strings. When a language construct has
semantic subparts, the grammar should expose those subparts as rules or tokens.
For example, directives are parsed as `directive_namespace`,
`directive_member`, and `directive_argument` instead of a flat string that later
needs to be split on dots.

String inspection is acceptable only as bounded recovery at integration points
where a dependency requires `&str`, or where editor input is malformed enough
that the CST cannot expose the missing child yet. Those fallback paths must
produce the same rule/range shape as the structured parser path so consumers do
not need separate language-specific parsing logic.

Semantic tokens, completions, hover, definition, formatting, lowering, and
checking should prefer CST/AST ranges and parser-owned structure. If an editor
feature needs to know whether the cursor is inside a namespace, member,
argument name, argument value, relation selector, or similar subpart, that
subpart should normally be represented in the grammar rather than inferred by
string manipulation.

## Language Atoms And Context

Language atoms colocate grammar ownership with construct-specific stage
behavior. An atom file should own the parsing/AST construction, formatting,
lowering, checking, language-service behavior, and no-effect declarations for
one source construct. `DirectiveAtom` is the current reference slice.
`DocumentAtom` owns the source root and is responsible for document-level AST
building, formatting, root completions, and document-syntax diagnostics while
still delegating child definitions to their own atoms.

Atoms are consumed through stage registries, not through ad hoc calls to a
specific atom. A stage should normally traverse the CST, AST, or lowered model,
identify the relevant grammar rule or typed node, and ask the atom registry for
the provider for that rule and stage. The stage supplies traversal context; the
atom supplies construct behavior. This keeps the caller from needing to know
whether a directive, fragment spread, field selection, or future construct owns
the current syntax.

Grammar rules are classified as one of:

- owned by an atom;
- delegated to an atom because the rule is a structural child of the owned
  construct;
- legacy, meaning it has not moved into an atom yet;
- internal, meaning it only groups syntax and has no direct feature owner.

Adding a grammar rule should force a classification decision in
`LanguageAtoms`. Rules that carry semantic or editor meaning should not be left
internal merely because no current stage consumes them.

Stage dispatch should follow this shape:

1. the consumer walks its native representation, for example CST nodes for
   formatting or AST/lowered nodes for semantic stages;
2. the consumer converts the current item to the parser rule or typed atom key;
3. `LanguageAtoms` returns the registered provider for that stage, or reports
   that the rule is still legacy/internal;
4. the consumer invokes the provider with normalized stage context.

Direct calls such as "run the directive checker here" are migration code unless
they are hidden inside the atom registry. The desired end state is that adding
or moving a construct changes the atom declaration and stage implementation,
not every traversal that might encounter that construct.

Atom-dispatched stages may use an `AssetRegistry` to provide shared compiler
assets and per-pass output capabilities without passing a broad project object
into atom implementations. The registry has two lifetimes:

- project assets are long-lived for a project/context/revision, for example a
  directive registry, external directive schemas, catalog snapshots, scoped
  fragments, and codegen options;
- atom assets are recreated for each stage pass, for example diagnostic
  stores, validation caches, completion scratch, and other accumulators.

Atoms should not receive the whole registry directly. They declare typed
parameters, and the stage descriptor extracts only those parameters from the
stage context. For example, directive completion can request
`(&LanguageContext, &DirectiveRegistry)` while field completion can request a
catalog and current table. Tuple extraction should be generated rather than
handwritten when this pattern grows.

Diagnostics can be emitted through atom assets such as
`DiagnosticStore<CheckDiagnostic>`. Atom implementations push diagnostics into
the requested store while their return value remains the semantic stage output.
The owning stage drains, sorts, deduplicates, and converts diagnostic stores at
the end of the pass. This keeps diagnostics centralized without forcing every
atom return type to carry side-channel data.

Picante memoization remains outside atom implementations. Tracked queries own
stage boundaries, build or receive stable project assets, create fresh atom
assets for the pass, dispatch pure atom stage functions, and drain atom assets
into explicit query outputs. Atoms consume materialized assets, not
`CompilerDb`, `ProjectHost`, `SourceDb`, or LSP state. If atom-level
memoization is added later, atom descriptors can declare memoization policy
while the Picante layer consumes those descriptors generically.

The document root is not project orchestration. `DocumentAtom` may own root
syntax behavior such as `query`/`fragment` completions and blank-line formatting
between definitions, but source loading, source-unit membership, scoped-program
construction, config resolution, and protocol adaptation remain frontend or
adapter responsibilities.

Language-service context has two phases:

1. `LanguageContextInput` records raw cursor evidence from the parse result:
   enclosing CST rules, containing token, expected tokens, and source position.
2. Atom context providers refine that input into generic `LanguageContext`
   values with a concrete `SyntaxRule`, evidence origin, confidence,
   `construct_range`, and `focus_range`.

The context provider is generic over syntax. It must not expose atom-specific
payload enums. An atom can use local helper structs while classifying, but the
published context should be expressed as syntax rules plus ranges. Completion,
hover, definition, semantic-token, and future editor consumers should match the
generic rule and read the supplied ranges.

Context providers should classify in this order:

1. exact CST evidence;
2. parser expected-token or recovery evidence;
3. bounded source-window fallback.

Fallback source-window classification is recovery only. It should be small,
cursor-local, and produce the same `SyntaxRule`/range shape as the CST path.
If a fallback grows into general parsing logic, the grammar likely needs a new
structural rule instead.

## Core Stages

Core analysis is organized as explicit stages:

1. Parse source into CST/AST plus parse diagnostics.
2. Lower syntax into interned semantic records where needed.
3. Extract definitions and fragment records.
4. Check definitions against catalog and visible fragments.
5. Lint checked source with catalog and lint options.
6. Plan checked selections for SQL generation.
7. Generate SQL, metadata, and adapter-facing models from planned data.
8. Format from the CST/source text when syntax is valid enough to rewrite
   safely.

Diagnostics are returned explicitly from stages and aggregated in source order
where possible. Diagnostics should carry precise byte ranges and stable
diagnostic source/category information.

Parsing, lowering, and definition extraction are context-free. Resolution
contexts are applied by frontend scoped analysis before context-aware checking,
linting, planning, generation, and presentation.

## ProjectHost

`ProjectHost` is the public project analysis facade used by LSP, validation,
generation, daemon-style project operations, and tests.

```rust
pub struct ProjectHost {
    sources: SourceDb,
    db: CompilerDb,
    contexts: DashMap<AnalysisContextId, Arc<AnalysisContextState>>,
    effective_contexts: DashMap<String, EffectiveResolutionContext>,
}
```

`ProjectHost` is cheaply clonable and shares internal state through the intended
frontend/query boundaries. It owns:

- source documents and source-unit membership through `SourceDb`;
- effective resolution contexts derived from project configuration;
- context source sets for scoped analysis;
- a Picante-backed `CompilerDb`;
- interactive analysis helpers for completion, hover, definition, formatting,
  semantic tokens, and diagnostics.

Project workflows should go through `ProjectHost`. Single-file developer
commands may still call core helpers directly when they intentionally do not
need project context.

## Resolution Contexts

Project configuration defines named resolution scopes. Effective resolution
contexts are built from a local scope plus its imported scopes. A shared source
unit can be visible in multiple contexts without being parsed as a different
file.

`ProjectHost` publishes context source sets into `CompilerDb` as
`ContextSourcesInput` values:

```rust
pub struct ContextSourcesInput {
    pub context: String,
    pub sources: Vec<ContextSource>,
}
```

`scoped_program_query` is the frontend query that builds the scoped definition
universe for one context. It collects context definitions, builds the visible
fragment list, and records scoped source-backed diagnostics such as duplicate
fragments.

Context handling should stay at this boundary. Lower-level parse and lower
queries should not know about resolution maps or imports.

## CompilerDb

`CompilerDb` is the Picante-backed query database in `dsql-frontend`.

Current tracked inputs include:

- `SourceInput`, keyed by `SourceUnitId` and source revision, storing
  a `SourceRegion`;
- `ContextSourcesInput`, keyed by context name;
- `CatalogInput`;
- `LintOptionsInput`.

Heavy parse/lower/extract/scoped outputs use `Arc<T>` where useful so cache hits
clone handles instead of large compiler data.

Current important queries:

- `parse_file(source) -> Arc<ParsedFile>`
- `lower_file_query(source) -> Arc<LoweredFile>`
- `extract_definitions_for_file(source) -> Arc<ExtractedFile>`
- `context_definitions_query(context) -> ContextDefinitions`
- `scoped_program_query(context) -> Arc<ScopedProgram>`
- `diagnostics_for_unit_in_scope(context, unit_id, source) -> Vec<Diagnostic>`
- `formatted_text_for_file(source) -> Option<String>`

`diagnostics_for_unit_in_scope` is the current scoped diagnostic aggregation
query. It fetches parse/lower/scoped/catalog/lint inputs, checks and lints the
unit in the scoped fragment universe, plans it, appends scoped diagnostics for
that unit, and sorts diagnostics by source range.

The current implementation still has standalone file-level check/lint/plan
queries for direct source analysis. Project diagnostics should use the scoped
path so imports and visible fragments are handled consistently.

## Catalog And Config Inputs

Catalog access goes through provider/loading boundaries outside core analysis,
then enters analysis as a catalog value. This keeps swapping a hardcoded catalog
for a PostgreSQL-backed catalog as a provider concern.

Implemented today:

- `CatalogInput` publishes the current catalog to `CompilerDb`;
- `LintOptionsInput` publishes lint configuration;
- project loading derives effective resolution contexts from `dsql.toml`.

Not yet implemented:

- a single normalized tracked `Config` input containing resolution, lint,
  formatting, generation, and catalog configuration;
- a revisioned `CatalogSnapshot` wrapper around `Arc<Catalog>`;
- selective invalidation keyed by config/catalog revision.

Until those inputs exist, code should still treat catalog and project config as
external immutable inputs at the frontend boundary.

## LSP And Editor Boundary

The LSP layer owns editor protocol conversion, not language rules. It should:

- update `ProjectHost`/`SourceDb` on open, change, and close;
- apply text edits through Rope range operations;
- request diagnostics, completion, hover, definition, formatting, and semantic
  tokens from `ProjectHost`;
- convert byte ranges to LSP positions at the boundary;
- avoid a second compiler document mirror.

`PresentedDiagnostic` maps compiler diagnostics back to physical documents,
embedded source ranges, source offsets, and line/character positions.

## Formatting

The formatter operates from the CST and original source spans. It must be
conservative:

- preserve comments and trivia unless a rule deliberately rewrites them;
- refuse formatting or preserve malformed regions when parse errors are
  present;
- avoid reconstructing output from AST data when source text should be
  preserved.

Formatter behavior should be covered by snapshots.
