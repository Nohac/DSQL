# Project Host, Source Snapshots, And Scoped Programs

## Summary

`ProjectHost` should be the only public analysis surface for project
loading, LSP/editor workflows, validation, and generation. It owns the project
source database, resolution environments, catalog/lint inputs, and the
Picante-backed compiler database. Older `AnalysisHost`/`FileId` bridge concepts
should be flattened into this model instead of being preserved as compatibility
surfaces.

The architecture has two separate graphs:

- a context-free source graph keyed by source units and Rope-backed snapshots;
- a scoped semantic graph keyed by resolution environments.

Parsing and lowering never depend on resolution contexts. A shared file imported
by `frontend` and `api` is parsed once, then reused by both scoped programs.
Resolution context is applied once at the `scoped_program(env)` boundary. All
downstream stages consume that already-scoped universe and should not know how
resolution maps or imports work.

## Terms

- **Physical document**: A file or editor buffer, such as
  `queries/movie.dsql` or `src/movie.tsx`.
- **Source unit**: One analyzable DSQL unit. A standalone `.dsql` file normally
  has one full-file unit. A TypeScript file can contain multiple embedded DSQL
  units.
- **Source unit ID**: The stable project/compiler identity for a source unit.
  This should replace the separate `FileId` bridge. If a compact numeric key is
  needed, `SourceUnitId` should be that key.
- **Source snapshot**: A cheap immutable analysis input containing a
  `SourceUnitId`, source metadata, and a cloned `Rope`.
- **Source DB**: The project-owned frontend source store that owns physical
  documents, source units, Rope state, source residency, and editor mutations.
- **Config**: The parsed project configuration derived from `dsql.toml`, editor
  overrides, or daemon input. It contains resolution maps/environments plus
  lint, formatting, generation, and adapter-relevant options in normalized
  analysis-facing form.
- **Resolution environment**: A config-derived rule describing one effective
  resolution world, for example `frontend` with visible scopes
  `["frontend", "shared"]`.
- **Scoped program**: The memoized semantic universe for one resolution
  environment. It contains visible source units, lowered definitions, definition
  indexes, fragment maps, and source-backed validation diagnostics.
- **Catalog snapshot**: An immutable, cheap-to-clone validation/planning input
  loaded by a catalog provider. It is published to the compiler DB with a
  revision so catalog changes invalidate catalog-aware stages.
- **Project host**: The public facade used by LSP, CLI, daemon, generation,
  and tests. It owns source DB + compiler DB + resolution environments +
  config/catalog inputs.

## Target Ownership

The target shape is one public facade:

```rust
ProjectHost {
    sources: SourceDb,
    config: Config,
    catalog: CatalogSnapshot,
    db: CompilerDb,
}
```

`AnalysisHost` should be deleted as a separate project/document ownership layer,
not retained as a compatibility wrapper. Code that still depends on host-owned
behavior should be refactored onto `ProjectHost`, `SourceDb`, or
`CompilerDb`, depending on which layer owns the behavior. Callers should not
choose between `ProjectHost` and `AnalysisHost`, and no private host-shaped
wrapper should preserve old behavior.

The source DB owns source identity and text residency:

```rust
SourceDb {
    documents: DashMap<PhysicalDocumentId, SourceDocument>,
    units: DashMap<SourceUnitId, SourceUnit>,
}

SourceDocument {
    id: PhysicalDocumentId,
    path: Option<PathBuf>,
    text: SourceText,
}

enum SourceText {
    Durable(Rope),
    Ephemeral(Option<Rope>),
}

SourceUnit {
    id: SourceUnitId,
    physical_document: PhysicalDocumentId,
    owner_scope: ScopeId,
    content_range: TextRange,
    source_offset: u32,
}
```

There should not be a separate compiler-owned map like:

```rust
file_ids: DashMap<SourceUnitId, FileId>,
next_file: AtomicU32,
```

Those fields exist only because the current code has two source identities.
`SourceUnitId` should be the compiler identity.

## Source Text Residency

Only two source text residency modes are needed:

```rust
enum SourceText {
    Durable(Rope),
    Ephemeral(Option<Rope>),
}
```

`Durable(Rope)` is retained by the source DB. It covers open editor buffers and
durable project source state that must remain available for LSP edits,
position conversion, formatting, or diagnostic presentation.

`Ephemeral(Some(Rope))` is a temporary source supply. When analysis asks for it,
the source DB transfers ownership to the compiler snapshot and stores
`Ephemeral(None)`.

```rust
impl SourceText {
    fn take_for_analysis(&mut self) -> Option<Rope> {
        match self {
            SourceText::Durable(rope) => Some(rope.clone()),
            SourceText::Ephemeral(slot) => slot.take(),
        }
    }
}
```

If analysis asks again for `Ephemeral(None)`, the source DB may reload the file
or reconstruct the unit text according to project policy:

```rust
match source.text {
    SourceText::Durable(rope) => rope.clone(),
    SourceText::Ephemeral(Some(rope)) => rope.take(),
    SourceText::Ephemeral(None) => load_rope_again(path_or_source_rule),
}
```

The compiler owns the Rope snapshot while parsing. After parse, the Rope should
be dropped unless a later result intentionally retains it. Long-lived source
text should not be stored twice as both `Rope` and `Arc<str>`.

## Rope Snapshots And Parser Input

Ropey `Rope` clones are cheap and share internal nodes using `Arc`; edits use
copy-on-write behavior. `Rope` also implements content-based `Eq`, `PartialEq`,
and `Hash`. This makes cloned Ropes a practical analysis snapshot carrier.

The compiler source input should be close to:

```rust
SourceSnapshot {
    unit_id: SourceUnitId,
    physical_document: PhysicalDocumentId,
    content_range: TextRange,
    source_offset: u32,
    rope: Rope,
}
```

The current Lelwel parser is `&str`-based, so `parse_source` cannot yet consume
Rope chunks directly. The intermediate target is:

```rust
fn parse_source(snapshot: SourceSnapshot) -> ParseResult {
    if let Some(text) = snapshot.as_contiguous_str() {
        Parser::new(text, ...).parse(...)
    } else {
        let text = Arc::<str>::from(snapshot.rope.to_string());
        Parser::new(&text, ...).parse(...)
    }
}
```

`SourceSnapshot::as_contiguous_str()` can return a borrowed `&str` when the
Rope has exactly one chunk. Multi-chunk Ropes fall back to one `Arc<str>`
allocation until the generated parser runtime supports chunked input.

Parser outputs should avoid retaining full source text long term. The eventual
parse result should store syntax structure, byte spans, diagnostics, and
source-unit identity. Lowering should extract or intern the text it needs for
semantic records. Source text should be read again from `SourceDb` only
at editor/presentation boundaries.

## Resolution Environments

Resolution environments are derived from `dsql.toml` and are invalidated only
when relevant project configuration changes.

```rust
ResolutionEnvironment {
    id: EnvId,
    local_scope: ScopeId,
    visible_scopes: Vec<ScopeId>,
}
```

For:

```toml
[resolution.shared]
documents = ["queries/shared/**/*.dsql"]

[resolution.frontend]
documents = ["src/**/*.tsx"]
imports = ["shared"]

[resolution.api]
documents = ["queries/api/**/*.dsql"]
imports = ["shared"]
```

the environments are:

```text
frontend = frontend + shared
api      = api + shared
```

`shared` is not its own effective environment unless project config makes it
one. Cyclic imports and unknown imported scopes should be rejected before any
analysis query is evaluated.

The environment rule is stable, but its source set can change:

- config changes invalidate environments;
- file inventory changes invalidate source units/source sets;
- editor edits invalidate source snapshots for affected units;
- embedded region extraction changes invalidate source units for the host file.

## Config, Catalog, And Validation Inputs

The catalog is a project-level validation/planning input. It should not live in
`SourceDb`, and `scoped_program(env)` should not bake catalog state into the
definition/fragment universe. `ProjectHost` owns the parsed project `Config`,
the catalog provider selected by that config, and the current immutable catalog
snapshot. It publishes `Config` and `CatalogSnapshot` to `CompilerDb` as tracked
inputs.

The target input shape should be cheap to clone:

```rust
Config {
    revision: ConfigRevision,
    environments: Vec<ResolutionEnvironment>,
    lint: LintConfig,
    formatting: FormatConfig,
    generation: GenerationConfig,
    catalog: CatalogConfig,
}

CatalogSnapshot {
    revision: CatalogRevision,
    catalog: Arc<Catalog>,
}

ValidationInputs {
    config: Config,
    catalog: CatalogSnapshot,
}
```

`CatalogProvider` remains the loading boundary. It can be hardcoded,
PostgreSQL-backed, test-provided, or daemon-provided, but once a catalog is
loaded it enters analysis as immutable tracked data. If the catalog provider
reloads schema information or the project switches catalog configuration,
`ProjectHost` publishes a new `CatalogSnapshot`. If `dsql.toml`, editor
overrides, or daemon-supplied config changes, `ProjectHost` publishes a new
`Config`.

Config changes should invalidate only the stages that depend on the changed
derived config fields:

- resolution map changes invalidate `resolution_environments()` and affected
  source sets/scoped programs;
- lint config changes invalidate lint diagnostics;
- formatting config changes invalidate formatting output;
- generation config changes invalidate generation models/artifact grouping;
- catalog config changes can select a new provider and therefore publish a new
  `CatalogSnapshot`.

Catalog changes should invalidate catalog-aware stages only:

- check/lint/plan roots;
- generation models derived from checked/planned roots;
- catalog-aware completion, hover, definition, and semantic-token models.

Catalog changes should not invalidate source-unit discovery, source snapshots,
parsing, lowering, or the catalog-free parts of `scoped_program(env)` such as
visible definitions and fragment maps. Source-backed scoped diagnostics such as
duplicate definitions, duplicate fragments, and unknown fragments remain scoped
program outputs. Catalog-backed diagnostics such as unknown tables, unknown
fields, relation failures, variable inference failures, and planning errors
belong to check/lint/plan queries that explicitly depend on `CatalogSnapshot`.

## Tracked Input Rule

Anything that can change outside Picante and affects compiler output must enter
the query graph as a tracked input. Do not pass mutable project state, provider
state, or adapter-owned state into tracked functions as ordinary references.

Use this rule:

- external mutable state becomes tracked input;
- compiler-derived state becomes tracked output;
- small stable identities or selections can be query keys.

Examples of tracked inputs include source snapshots, parsed project `Config`,
catalog snapshots, open document text/revisions, source-unit membership, and
catalog/provider revisions. Examples of stable keys include `SourceUnitId`,
`EnvId`, `PhysicalDocumentId`, cursor-context IDs, and byte ranges.

Long-running LSP and watch-mode generation should update analysis by publishing
new immutable input values:

```rust
host.update_config(new_config);
host.update_catalog(new_catalog_snapshot);
host.update_source_snapshot(unit, new_snapshot);
```

Those methods may replace host-side handles in place, but they must publish new
tracked input values to `CompilerDb`. They must not mutate fields inside an
existing `Config`, `CatalogSnapshot`, or source snapshot that a tracked query may
already have observed. With stable equality/hash/Facet semantics on input values,
Picante can naturally keep unaffected query outputs cached and invalidate only
queries that depend on the changed input.

## Query Graph

The intended graph is:

```mermaid
flowchart TD
  RawConfig[dsql.toml/editor/daemon config] --> Config[config()]
  Config --> Envs[resolution_environments()]
  CatalogProvider[catalog provider] --> Catalog[catalog_snapshot()]
  Config --> LintConfig[lint_config()]
  Config --> FormatConfig[format_config()]
  Config --> GenConfig[generation_config()]
  SourceDb[SourceDb documents + ropes] --> Units[source_units()]
  Units --> Snapshot[source_snapshot(unit_id)]
  Snapshot --> Parse[parse_source(snapshot)]
  Parse --> Lower[lower_source(snapshot)]
  Lower --> Scoped[scoped_program(env_id)]
  Envs --> Scoped
  Units --> Scoped
  Scoped --> Check[check_roots(scoped_program)]
  Scoped --> Lint[lint_roots(scoped_program)]
  Scoped --> Plan[plan_roots(scoped_program)]
  Catalog --> Check
  Catalog --> Lint
  Catalog --> Plan
  Catalog --> Gen
  LintConfig --> Lint
  GenConfig --> Gen
  Scoped --> Gen[generation_model(scoped_program)]
  Check --> Diags[diagnostics_for_env(env_id)]
  Lint --> Diags
  Plan --> Diags
  Gen --> Diags
  Diags --> Presented[presented_diagnostics(document)]
```

The only context-aware semantic query is:

```rust
scoped_program(env_id) -> ScopedProgram
```

`scoped_program` knows how `frontend` imports `shared`. Downstream stages do
not. They only see an already-scoped program:

```rust
ScopedProgram {
    env: EnvId,
    source_units: Vec<SourceUnitId>,
    lowered_files: Vec<LoweredFileRef>,
    definitions: DefinitionIndex,
    fragments: FragmentMap,
    diagnostics: Vec<Diagnostic>,
}
```

Downstream APIs should be context-blind:

```rust
check_file(scoped, unit, catalog)
lint_file(scoped, unit, catalog, config.lint)
plan_query(scoped, query, catalog)
generation_model(scoped, catalog, config.generation)
diagnostics_for_unit(scoped, unit)
```

This prevents LSP, validation, and generation from growing parallel
context-specific rule implementations. These stages can be catalog-aware, but
they should stay resolution-map-blind because `scoped` already contains the
visible semantic universe.

## Memoization And Invalidation

| Stage | Key | Depends On | Invalidated By | Context Aware |
|---|---|---|---|---|
| `config()` | config revision | `dsql.toml`, editor overrides, daemon config input | project/editor/daemon config change | no |
| `resolution_environments(config)` | config revision | config resolution maps/imports | resolution/import config changes | yes, config only |
| `source_units()` | source DB inventory revision + selectors | physical files, open buffers, embedding extraction | file add/remove, open/close, embedded region shape change | scope ownership only |
| `source_snapshot(unit)` | source unit ID + Rope content/revision | current durable/open/ephemeral text | text edit, file reload, ephemeral reload | no |
| `parse_source(snapshot)` | source snapshot content | cloned Rope / fallback `Arc<str>` | snapshot content change | no |
| `lower_source(snapshot)` | parse result | CST/spans | parse invalidation | no |
| `catalog_snapshot()` | catalog revision | provider-loaded catalog | schema reload, catalog config change | no |
| `scoped_program(env)` | env ID + visible source unit set + lowered revisions | env, source units, lowered definitions | env change, source-unit membership change, lowered definition change | yes, single boundary |
| `check_roots(scoped, catalog)` | scoped program revision + catalog revision | scoped definitions, fragment map, catalog | scoped program/catalog change | no context rules |
| `lint_roots(scoped, catalog, config.lint)` | scoped program revision + catalog/config revisions | scoped program, catalog, lint config | scoped program/catalog/lint config change | no context rules |
| `plan_roots(scoped, catalog)` | scoped program revision + catalog revision | scoped program, catalog | scoped program/catalog change | no context rules |
| `format_edits(snapshot, config.formatting)` | source snapshot + config revision | CST/source text, formatting config | source/config change | no |
| `generation_model(scoped, catalog, config.generation)` | scoped program revision + catalog/config/planning revision | checked/planned definitions, generation config | scoped/check/plan/catalog/generation config change | no context rules |
| `presented_diagnostics(document)` | document ID + diagnostics revision | source DB positions, scoped diagnostics | source text or diagnostics change | adapter only |

Parser reuse across environments is automatic because `parse_source(snapshot)`
is keyed by the source snapshot, not by environment.

Example:

```text
queries/shared/title-fragments.dsql
  SourceUnitId(42), owner_scope = shared

frontend env sees: [frontend units..., SourceUnitId(42)]
api env sees:      [api units...,      SourceUnitId(42)]
```

Both environments use the same query:

```rust
parse_source(snapshot_for(SourceUnitId(42)))
lower_source(snapshot_for(SourceUnitId(42)))
```

Only `scoped_program(frontend)` and `scoped_program(api)` differ.

## Diagnostics

Diagnostics should be produced by compiler queries as byte ranges over source
units. They should not carry full source text.

Source-backed validation failures such as duplicate anonymous variables,
anonymous queries, duplicate definitions, duplicate fragments, and unknown
fragments belong in `scoped_program` or directly derived scoped diagnostics.
Catalog-backed failures such as unknown tables, unknown fields, relation
failures, variable inference failures, and planning errors belong in
check/lint/plan diagnostics that depend on `CatalogSnapshot`. Both groups must
surface through the same `PresentedDiagnostic` path for LSP, `dsql validate`,
daemon responses, and generation.

Presentation is an adapter boundary:

```rust
ProjectHost::diagnostics_for_document(document)
  -> collect diagnostics from every environment containing the document's units
  -> map unit ranges to physical document byte ranges
  -> derive line/column from SourceDb Rope state when available
  -> include env label only when multiple effective envs exist
```

Ephemeral source units can produce diagnostics, but if their text is not
retained by the source DB then presentation must either use information already
stored in the diagnostic result or reload the source. Durable/open documents
should use retained Ropes for line/column conversion.

## LSP Integration

The LSP server should own `ProjectHost`.

Open/change/close flow:

```text
open document
  -> SourceDb stores Durable(Rope)
  -> source units are refreshed
  -> affected source snapshots are invalidated
  -> diagnostics are requested from ProjectHost

change document
  -> SourceDb mutates the retained Rope
  -> source units are refreshed
  -> affected snapshots/scoped programs are invalidated
  -> diagnostics are republished for open affected documents

close document
  -> SourceDb demotes, reloads, or evicts according to policy
  -> affected source units/scoped programs are refreshed
```

Interactive requests select a source unit and then a deterministic environment:

```text
cursor position
  -> source unit at byte
  -> environments containing that source unit
  -> active editor-selected env if available
  -> otherwise deterministic fallback
  -> use scoped_program(env) for completion/hover/definition/semantic tokens
```

`ProjectHost` should centralize that setup in one helper used by completion,
hover, definition, semantic tokens, and formatting:

```rust
fn interactive_analysis(uri, position) -> Result<InteractiveAnalysis> {
    let request = resolve_interactive_position(uri, position)?;
    let snapshot = db.source_snapshot(request.unit);
    let parsed = db.parse_source(snapshot.clone());
    let lowered = db.lower_source(snapshot.clone());
    let scoped = db.scoped_program(request.env);
    let catalog = db.catalog_snapshot();

    Ok(InteractiveAnalysis {
        request,
        snapshot,
        parsed,
        lowered,
        scoped,
        catalog,
    })
}
```

This helper should not become a second analysis pipeline. It only maps editor
input to a source unit/environment and fetches the tracked compiler outputs that
interactive features need. Feature-specific code should consume
`InteractiveAnalysis` and return compiler-level models such as completion
candidates, hover content, definition locations, or semantic token spans. LSP
conversion remains at the protocol boundary.

Catalog-aware interactive features should read from `InteractiveAnalysis.catalog`
rather than asking a provider directly or using an adapter-local catalog copy.

`InteractiveAnalysis` should be an internal, request-scoped carrier. It can own
cheap query handles such as `Arc<ParsedFile>`, `Arc<LoweredFile>`, and
`Arc<ScopedProgram>`. Picante 2.0 and 3.0.0-rc.0 store derived query values
internally behind `Arc<dyn Any>`, but the generated tracked wrapper returns
owned `V`; on a cache hit it downcasts the cached `Arc<V>` and returns `V`,
cloning `V` if the cached cell still owns a reference. For large compiler
outputs, make the tracked output type itself a cheap handle, for example
`PicanteResult<Arc<ParsedFile>>` rather than `PicanteResult<ParsedFile>`.
Input fields should follow the same rule: use cheap field values such as
`Arc<str>` or `Rope`, not large owned structs that must be cloned on every field
read. Public APIs should return owned
compiler-level models and should not expose database-borrowed references to
LSP, CLI, daemon, or generation adapters. Avoid self-referential shapes where
`parsed` borrows from a `snapshot` stored in the same carrier.

The raw byte offset is a hit-testing input, not the ideal cache key for heavy
interactive results. When those results become tracked, the helper should derive
a stable cursor context from `parsed`/`lowered` first, so moving within the same
symbol reuses the same completion, hover, or definition result.

The editor should not merge semantic worlds from multiple environments.

## Generation And TypeScript Integration

Generation iterates scoped programs:

```text
catalog = catalog_snapshot()
for env in resolution_environments:
  scoped = scoped_program(env)
  fail on error diagnostics
  model = generation_model(scoped, catalog)
  emit artifacts for env
```

Generation must not parse, lower, or assemble fragment maps independently.
Generation must not reload or reinterpret catalog/schema independently either;
it should consume the same tracked `CatalogSnapshot` used by validation and
planning. Generated artifact groups should correspond to effective resolution
environments. TypeScript renderers can no-op omitted environments by simply not
including them in their local scope-to-path map.

## Migration Plan

1. Add `SourceUnitId`, `SourceSnapshot`, and `SourceText::{Durable, Ephemeral}`.
2. Move source-unit identity into `SourceDb`; remove separate
   `SourceUnitId -> FileId`/`next_file` bridges.
3. Make `CompilerDb`/Picante queries accept source snapshots keyed by
   `SourceUnitId` and Rope content/revision.
4. Update `parse_source` to prefer contiguous Rope `&str` and fall back to
   `Arc<str>` for multi-chunk Ropes.
5. Stop long-lived parse/lower results from retaining full source text where
   practical; intern or copy semantic names during lowering.
6. Add parsed `Config` and `CatalogSnapshot` tracked inputs owned by
   `ProjectHost` and loaded from config/provider boundaries. `Config` includes
   derived `ResolutionEnvironment` records plus lint, formatting, generation,
   and catalog provider config.
7. Add `scoped_program(env_id)` as the only query that understands resolution
   imports and visible source units.
8. Move duplicate/unknown/source-backed validation diagnostics into scoped
   program outputs.
9. Make check/lint/plan/generation and catalog-aware interactive features
    consume `CatalogSnapshot` instead of reading catalog providers directly.
10. Rework LSP diagnostics, completion, hover, definition, semantic tokens, and
    formatting to select an environment and consume scoped programs.
11. Rework generation to consume scoped generation models only.
12. Delete `AnalysisHost` as a separate layer and move any remaining behavior
    into `ProjectHost`, `SourceDb`, or `CompilerDb`.

## Acceptance Criteria

- `ProjectHost` is the only public analysis API used by CLI, LSP, daemon,
  generation, and integration tests.
- Source identity is `SourceUnitId`; there is no separate public `FileId`
  allocator for project source units.
- `SourceDb` owns durable/open source Ropes and can transfer ephemeral
  Ropes into compiler snapshots.
- Parser/lower results are memoized by source snapshot, not by resolution
  environment.
- A shared source unit imported by multiple environments is parsed and lowered
  once per content snapshot.
- Resolution imports are applied once in `scoped_program(env)`.
- Check, lint, plan, diagnostics, and generation consume scoped programs and do
  not implement resolution-map rules.
- Parsed `Config` and `CatalogSnapshot` enter `CompilerDb` as tracked inputs at
  the validation/planning/formatting/generation boundaries.
- Catalog changes invalidate check/lint/plan/generation and catalog-aware
  interactive models without invalidating parse/lower/source-unit membership.
- Config changes invalidate only the stages that depend on the changed derived
  config fields, such as environments, lint diagnostics, formatting output, or
  generation models.
- Diagnostics for LSP and `dsql validate` come from the same scoped diagnostic
  carrier.
- TypeScript generation receives per-environment artifact groups without
  duplicate frontend parsing or scope assembly.
