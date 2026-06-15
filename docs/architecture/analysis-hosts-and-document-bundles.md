# Analysis Hosts And Document Bundles

## Summary

The compiler architecture should treat an analysis host as a memoized compiler
runtime for the documents it is given. Project loading is responsible for
constructing document bundles, creating one analysis host per bundle, and
carrying a context label so diagnostics and generated artifacts can explain
which bundle produced them.

This keeps the analysis host small: it does not need to understand project
resolution maps, scope semantics, or why a document belongs to a bundle. It only
analyzes the source regions it receives and returns structured outputs tagged
with the context supplied by the caller.

## Terms

- **Physical document**: A file or editor buffer, such as `queries/movie.dsql`
  or `src/movie.tsx`.
- **Source region**: A DSQL analyzable region inside a physical document. A
  standalone `.dsql` file has one full-file region. A TypeScript file can have
  multiple embedded regions.
- **Document bundle**: The set of source regions that should be analyzed
  together for one generation or resolution context.
- **Analysis context**: A stable identifier and display label for one document
  bundle. Existing resolution map names are one source of context labels.
- **Resolution import graph**: The directed graph formed by resolution maps and
  their `imports`. An edge points from the consuming resolution map to the
  resolution map it imports.
- **Effective analysis context**: A resolution map that is not imported by any
  other resolution map. It gets an analysis host containing its own source
  regions plus the source regions from every transitive import.
- **Imported scope**: A resolution map that is consumed only through another
  resolution map. It contributes source regions to effective contexts but does
  not get its own analysis host unless it is also an effective context.
- **Analysis host**: A Picante-backed compiler state instance for one document
  bundle.
- **Project source DB**: The frontend/project source store that owns Rope-backed
  physical document state, document residency, and revision publication into
  affected analysis hosts.
- **Project analysis**: The project-level layer that loads project config, resolves
  document globs and embedded regions, builds document bundles, owns the map of
  context labels to analysis hosts, and adapts outputs for CLI, daemon, LSP, and
  generation.

## Architecture

`AnalysisHost` is kept as the host name because it matches the existing frontend
API and the rust-analyzer convention of a mutable owner for incremental analysis
state. The project-level type should be named `ProjectAnalysis` rather than
`ProjectCoordinator`: it is the analysis-facing entrypoint for a loaded project,
not a general orchestration service.

`ProjectAnalysis` owns project-level routing and context construction:

```text
load project
  -> read config, catalog, lint options
  -> resolve physical documents in ProjectSourceDb
  -> extract DSQL source regions
  -> build document bundles
  -> create one AnalysisHost per bundle
  -> insert that bundle's source regions into the host
  -> run diagnostics / generation per host
  -> format outputs at the edge
```

An analysis host owns only the documents it was given:

```rust
AnalysisHost {
    context: AnalysisContext,
    db: CompilerDb,
}

AnalysisContext {
    id: AnalysisContextId,
    label: String,
}
```

The host does not resolve imports or decide membership. If a shared source file
must participate in multiple bundles, `ProjectAnalysis` inserts the same source
region into multiple hosts. This is intentional: the same source can produce
different diagnostics in different contexts.

Resolution maps are interpreted as a directed acyclic import graph. For
example, if `api` and `frontend` both import `shared`, the project layer creates
hosts for `api` and `frontend` only:

```text
api host      = api + shared
frontend host = frontend + shared
```

`shared` is an imported scope in this example, not a standalone analysis host.
Project loading must reject cyclic resolution imports before any hosts are
created.

`ProjectAnalysis` should own a source DB rather than scattering source state
across the LSP, CLI, daemon, and generation adapters:

```rust
ProjectAnalysis {
    sources: ProjectSourceDb,
    hosts: DashMap<AnalysisContextId, AnalysisHost>,
}

ProjectSourceDb {
    entries: DashMap<PhysicalDocumentId, SourceEntry>,
}

SourceEntry {
    id: PhysicalDocumentId,
    path: Option<PathBuf>,
    revision: RevisionId,
    rope: Rope,
    residency: SourceResidency,
}

enum SourceResidency {
    AnalysisSnapshot,
    OpenEditable,
}
```

This source DB is a frontend/project boundary component, not part of the pure
compiler core. It is allowed to use internal mutability with `DashMap` because
it owns mutable project/editor state. The pure language stages still receive
explicit immutable inputs through the analysis host/query boundary.

`AnalysisSnapshot` entries are loaded to analyze a project, are comparatively
stable, and may be evicted or reloaded by project policy. `OpenEditable` entries
represent live editor buffers; they are updated by Rope range operations and
publish new immutable revisions into every host containing affected source
regions. If an on-disk project file becomes open in the editor, the source DB
should promote that physical document to `OpenEditable` instead of maintaining a
second source copy.

Filesystem watchers, editor document events, or other project-level invalidation
sources should live at this project/source-DB boundary. They update
`ProjectSourceDb` first, then ask `ProjectAnalysis` which analysis hosts contain
the changed source regions and publish new host inputs only to those hosts.

## Diagnostics

Diagnostics are owned by the analysis host and stored as part of its Picante
query outputs. Host-owned diagnostics do not need to store the analysis context:
the host already has that identity. The project layer attaches context labels
when diagnostics are collected for an edge surface.

```rust
HostDiagnostic {
    file: PathBuf,
    source_offset: u32,
    range: TextRange,
    severity: Severity,
    source: DiagnosticSource,
    code: DiagnosticCode,
    message: String,
}
```

This is the host-owned diagnostic shape. Edge adapters can derive presentation
diagnostics from it. The stored diagnostic itself should not contain context
labels, formatted line/column strings, or protocol-specific fields.

```rust
PresentedDiagnostic {
    context: AnalysisContext,
    file: PathBuf,
    range: TextRange,
    start_position: Option<SourcePosition>,
    severity: Severity,
    source: DiagnosticSource,
    code: DiagnosticCode,
    message: String,
}
```

Diagnostics must not carry full source text. Source text and Ropes are owned by
`ProjectSourceDb` when retained. Byte ranges remain the internal location
format. Line and column are derived by formatting against the source DB's
Rope-backed state.

The formatting boundary should be explicit. The exact API can evolve, but the
shape should be close to:

```rust
impl HostDiagnostic {
    /// Formats this diagnostic for a protocol or display surface using the
    /// source DB and context supplied by the analysis host or project layer.
    pub fn format(&self, context: AnalysisContext, sources: &ProjectSourceDb) -> PresentedDiagnostic;
}
```

When multiple effective contexts report diagnostics for the same open document,
adapters should keep the consuming context visible in the presented diagnostic.
For LSP this can be done with the diagnostic source, code metadata, or message
text because LSP diagnostics do not have a native scope dimension. If a project
has only one effective context, adapters should present diagnostics without a
context label; the label only carries useful information once two or more
contexts can report different results for the same source.

## Root Definition Index

Each host should build one authoritative root definition index for its
document bundle. Query-specific and fragment-specific views should be derived
from that index instead of maintaining unrelated maps.

```rust
DefinitionIndex {
    definitions: Vec<RootDefinition>,
    named: HashMap<String, RootDefinitionBucket>,
}

Root<T> {
    id: DefinitionId,
    source: SourceRegionId,
    ordinal: u32,
    name: Option<String>,
    name_range: Option<TextRange>,
    range: TextRange,
    inner: T,
}

type QueryRoot = Root<QueryRootInner>;
type FragmentRoot = Root<FragmentRootInner>;

QueryRootInner {
    record: QueryRecord,
}

FragmentRootInner {
    record: FragmentRecord,
}

RootDefinition {
    Query(QueryRoot),
    Fragment(FragmentRoot),
}

RootDefinitionBucket {
    primary: DefinitionId,
    conflicts: Vec<DefinitionId>,
}
```

`Root<T>` stores fields shared by every root definition exactly once. Typed
aliases such as `QueryRoot` and `FragmentRoot` keep query- and fragment-specific
payloads explicit without duplicating source metadata. Implementing `Deref` or
small accessor methods on `Root<T>` is acceptable if it keeps call sites focused
on the definition payload while preserving access to the shared root metadata.

The `definitions` vector preserves deterministic source-order iteration. The
`named` map indexes only named definitions and provides O(1) lookup from
definition name to the bucket for that name. Anonymous root definitions remain
in `definitions` but are not inserted into `named`; generation validation can
diagnose them later when a target requires named roots. The bucket keeps the
first definition as `primary` and records later conflicts in source order. While
building the map, inserting a name that already exists should produce a
duplicate-definition diagnostic using the later definition's `name_range` when
available.

The index should expose convenience methods for common typed views:

```rust
impl DefinitionIndex {
    /// Returns the primary named root definition in constant time when it exists.
    pub fn get(&self, name: &str) -> Option<&RootDefinition>;

    /// Returns the named root definition bucket in constant time when it exists.
    pub fn bucket(&self, name: &str) -> Option<&RootDefinitionBucket>;

    /// Returns every root query in deterministic source order.
    pub fn queries(&self) -> impl Iterator<Item = QueryDefinitionRef>;

    /// Returns every root fragment in deterministic source order.
    pub fn fragments(&self) -> impl Iterator<Item = FragmentDefinitionRef>;

    /// Returns queries addressable by name using the name bucket.
    pub fn queries_by_name(&self, name: &str) -> impl Iterator<Item = QueryDefinitionRef>;

    /// Returns fragments addressable by name using the name bucket.
    pub fn fragments_by_name(&self, name: &str) -> impl Iterator<Item = FragmentDefinitionRef>;
}
```

Duplicate definition diagnostics, fragment lookup, generation grouping, and
future root definition kinds should all build on this shared index.

## Picante Query Shape

Each analysis host uses Picante to memoize work for its own document bundle:

```text
parse_region(region_id) -> parsed CST/AST and parse diagnostics for one source region
lower_region(region_id) -> lowered source structure for one parsed region
definitions_for_region(region_id) -> root definitions extracted from one region
definition_index() -> merged root definition index for this host's bundle
fragment_map() -> fragment lookup view derived from the definition index
check_definition(definition_id) -> check diagnostics for one root definition
lint_definition(definition_id) -> lint diagnostics for one root definition
plan_definition(definition_id) -> query/fragment plan output for one definition
diagnostics_for_region(region_id) -> diagnostics from all stages affecting one region
diagnostics_for_host() -> all diagnostics for this host/context
generation_model() -> validated, planned definitions ready for artifact generation
```

Stages remain Picante-oblivious. Picante queries assemble explicit inputs and
call pure compiler functions:

```text
query check_definition(definition_id):
  definition = definition_index().get(definition_id)
  fragments = fragment_map()
  catalog = catalog_input()
  check_query_definition(&definition, &fragments, &catalog)
```

This permits incremental recomputation inside each host. A changed source region
invalidates that region's parse/lower/definition outputs in every host that
contains it, but unaffected regions and definitions in those hosts remain
memoized.

## Project Loading And Bundles

Project loading should convert config into physical documents and document
bundles before analysis:

```rust
PhysicalDocument {
    id: PhysicalDocumentId,
    path: PathBuf,
    revision: RevisionId,
}

DocumentBundle {
    context: AnalysisContext,
    regions: Vec<ProjectSourceRegion>,
}

ProjectSourceRegion {
    physical_document: PhysicalDocumentId,
    content_range: TextRange,
    source_offset: u32,
}
```

`PhysicalDocument` is the stable identity and revision metadata for a file or
editor buffer. Source ownership lives in `ProjectSourceDb`; document and region
metadata should not duplicate it. `ProjectSourceRegion` points into the physical
document by byte range. Standalone `.dsql` files use a full-file
`content_range`; embedded TypeScript regions use the extracted DSQL range.

When source text is retained, the retained representation should be a `Rope`.
Do not store both a `Rope` and an `Arc<str>`/`String` for the same long-lived
source snapshot. `&str`, `String`, and `Arc<str>` should be used only at edges
that cannot accept a Rope, Rope slices, chunks, readers, or iterators. Batch
project loading may create `AnalysisSnapshot` Rope entries, feed the analysis
host from those Rope entries, and evict them later if no presentation boundary
or incremental workflow still needs them.

Resolution maps can be implemented as bundle construction rules. The analysis
host does not need to know that a bundle came from a resolution map; it only sees
the final set of regions.

Bundle construction should first validate the resolution import graph:

- every imported resolution map must exist;
- cyclic imports are rejected;
- effective contexts are the resolution maps that no other map imports;
- when no explicit resolution maps exist, the project has one default effective
  context;
- each effective context bundle contains the context's local regions plus all
  transitive imported regions.

If a shared file is included by multiple bundles, it is inserted into multiple
hosts. That allows context-specific diagnostics such as duplicate fragment or
duplicate generated operation errors to surface only for the bundles where they
actually occur.

## State And Concurrency

This architecture should preserve the current preference against
`Arc<Mutex<_>>` and `Arc<RwLock<_>>` as the normal application shape.

`ProjectAnalysis` can own analysis hosts in a concurrent map:

```rust
ProjectAnalysis {
    sources: ProjectSourceDb,
    hosts: DashMap<AnalysisContextId, AnalysisHost>,
}
```

`AnalysisHost` should remain cheaply clonable and should rely on Picante/runtime
internal mutability rather than external locks. `ProjectSourceDb` can use
`DashMap` to coordinate physical document state, source residency, source
regions, and affected hosts. If the LSP needs to update every host that contains
an edited source region, it mutates the Rope in `ProjectSourceDb`, publishes a
new immutable revision, and updates the hosts found through the project indexes.

## LSP Integration

The LSP server should talk to `ProjectAnalysis`, not directly to arbitrary
analysis hosts.

For open/change/close:

```text
edit physical document
  -> ProjectSourceDb applies Rope range operations
  -> ProjectAnalysis updates affected source regions
  -> publish new immutable revisions into every host whose bundle contains those regions
```

For hover/completion/definition:

```text
cursor in physical document
  -> find source region at byte
  -> find contexts containing that region
  -> choose context from editor state if one is active
  -> otherwise use a deterministic default or return merged context-aware data
```

For diagnostics:

```text
publish diagnostics for physical document
  -> collect diagnostics from every host containing regions from that document
  -> map embedded ranges back to the physical document
  -> derive line/column from ProjectSourceDb Rope state
  -> include context label only when more than one effective context exists
```

If the editor later gains an active-context selector, LSP can filter diagnostics
and semantic features to that context. Until then, publishing all context-labeled
diagnostics is the clearest behavior.

## Generation And Daemon Integration

Generation runs per analysis host:

```text
for bundle in project.document_bundles:
  host = project.host(bundle.context)
  diagnostics = host.diagnostics()
  fail if error diagnostics
  model = host.generation_model()
  emit artifacts for bundle.context
```

The daemon should serialize structured diagnostics from the host/project layer. It
should not reconstruct source positions from file reads or source strings stored
on diagnostics. If line/column is needed in the daemon response, it should be
computed from `ProjectSourceDb` Rope state before serialization.

TypeScript and Vite should format diagnostics only. They may prefer
`file:line:column` when supplied and fall back to byte ranges when not supplied,
but they should not open files to derive positions.

## Migration Plan

1. Remove transient diagnostic source-text plumbing and any daemon `&str`
   byte-to-position helpers.
2. Add `AnalysisContext` labels to analysis host construction and diagnostic
   output.
3. Add `ProjectSourceDb` to own Rope-backed physical document state, source
   residency, and revision publication.
4. Add `ProjectAnalysis` to resolve project documents into
   `DocumentBundle`s using `ProjectSourceDb`.
5. Create one Picante-backed `AnalysisHost` per bundle and insert each bundle's
   source regions into it.
6. Make generation consume `AnalysisHost::generation_model()` instead of
   reparsing and regrouping project documents.
7. Make LSP diagnostics collect from all hosts that include the edited physical
   document, deriving positions from `ProjectSourceDb` and preserving context
   labels.
8. Move duplicate fragment/query diagnostics into host-level definition indexes
   so they are naturally context-specific.
9. Remove generation-local duplicated parsing, extraction, and scope assembly
   after behavior is covered by tests.

## Acceptance Criteria

- Analysis hosts analyze only the documents they are given.
- Project resolution maps are expressed as document-bundle construction, not as
  language-stage behavior.
- Project resolution imports are validated as an acyclic graph.
- Only effective consuming contexts receive analysis hosts; imported-only scopes
  are inserted into their consumers' hosts.
- Retained project/editor source state is owned by `ProjectSourceDb` as Ropes
  with explicit `AnalysisSnapshot` or `OpenEditable` residency.
- The architecture does not keep duplicate long-lived `Rope` and
  `Arc<str>`/`String` representations for the same source snapshot.
- The same physical document can participate in multiple hosts and produce
  different diagnostics per context.
- Diagnostics include source byte ranges, but not full source text. Presented
  diagnostics include context identity only when more than one effective context
  exists.
- LSP can publish context-labeled diagnostics for one physical document.
- Generation loops analysis hosts/bundles and does not duplicate frontend
  parsing or definition extraction logic.
- Edge adapters format diagnostics; they do not recover compiler state by
  rereading source files.
