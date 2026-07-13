- lsp server: unnecessary session state, after any run, just ask porridge for diags and broadcast to all files with a "durable" lifetime (open in lsp), needs durability system.
- Source durability, if open for analysis only, consume at "parse" time, if open through lsp, only materialize at parse time, but keep rope in memory.

# Design question

- DefDecl: is this the correct abstraction, what is DefDecl vs the Ast, isn't lowering supposed to generate Ast nodes? These separations seems completely backwards/redundant? DefKind is just a flat enum, where is the rest of the data etc?
- ~~Are the stage traits (LowerStage, HoverStage etc) even necessary anymore when we have systems and plugins?~~ RESOLVED 2026-07-11: Hover/CompletionStage retired into plain registration; LowerStage/FormatStage stay (exhaustive rule ownership).

# Tracked follow-ups

- Imported-query collisions are not diagnosed: `check_import_collisions`
  covers fragments only, so a local query colliding with an imported query
  (or two imports providing the same query name) passes the language
  checks and only surfaces as a generate-boundary artifact collision.
- Artifact paths are flat per kind; docs/spec/resolution-scopes.md calls
  for scope-qualified artifact groups, which will let independent scopes
  keep identical operation names without generate-boundary collisions.
- **Cross-file fragment invalidation is file-fingerprint-coarse** (the
  staleness bug found 2026-07-11 is FIXED: `DefIndex` fragment entries
  now carry their defining file's content fingerprint, pinned by
  `sql::cross_file_fragment_body_edits_rederive_sql`): any fragment-file
  edit re-runs every definition's check/variable/plan walks (~50ms per
  keystroke in fragment files on the reference project, vs ~32ms
  elsewhere). The finer-grained design — per-(definition,
  expanded-fragment) tracked pairs following `resolution.rs`, or an
  engine feature fingerprinting relationship members' content (subtree
  fingerprints) — recovers per-fragment granularity and belongs with the
  walk decomposition work.

- **Directive semantics dispositions (2026-07-12, codex-flagged, owner
  sign-off wanted)**: the registry validates both system directives
  fully, but `@dsql.include_if` additionally errors ("recognized but its
  semantics are not implemented yet") because accepting it while the
  planner ignores it would generate silently-unconditional SQL — the POC
  accepted it with empty planner/metadata impls, which we judged a
  correctness bug rather than parity to preserve. `@dsql.deprecated` is
  accepted without any metadata flow (exactly the POC's behavior; the
  annotation changes no output). Lifting the include_if error requires
  planner support for conditional selections.

# Deferred designs (owner input wanted)

- **Source residency & representation** (review High 5, and the eviction
  brainstorm from 2026-07-09): split identity from parse materialization —
  cheap revision fingerprints for editor-owned ropes vs content
  fingerprints for derived regions; regions as host/range facts
  materialized at the parser boundary instead of copied ropes; an
  analysis-only residency that can release mutable input after outputs
  are secured.

  **Owner design (Jonas, 2026-07-13) — hash-carrying evictable ropes.**
  `SourceText` becomes `{ rope: Option<Rope>, hash }`: the hash is
  computed when the rope is loaded and stored beside it, and the manual
  `Hash` impl hashes the *stored hash only*, never the rope. Evicting
  the rope (`rope = None`) after the parse consumes it therefore does
  not change the component's hash — no revision bump, every downstream
  fact stays valid, and the `DerivedFrom` anchor-bump problem never
  arises because nothing is removed, only the payload drops. Readers of
  an evicted rope either get `None` or the accessor reloads it from
  disk on demand (same hash → still no bump; a hash mismatch on reload
  means the file changed and *should* bump). Residency is per-origin:
  - **analysis-only** (CLI check/sql/generate): consume at parse time,
    set the rope to `None`, keep the hash;
  - **LSP-owned buffers**: never evicted — the buffer is the source of
    truth and cannot be reloaded from disk, so the rope stays resident
    and the stored hash is re-derived from the rope on each edit.

  Remaining sub-questions: where eviction is triggered (parser boundary
  vs an explicit post-settle sweep), and the separate region-facts idea
  (regions as host/range facts materialized at the parser boundary
  instead of copied ropes), which this design does not depend on.
- **Per-file diagnostics demand (debounce)**: demand rows per open file
  instead of the global singleton, so adapters can drop/re-arm per
  keystroke. Interacts with the check systems' demand joins and the walk
  decomposition below.
- **Walk decomposition**: per-(definition, expanded-fragment) tracked
  pairs (the `resolution.rs` pattern) or engine subtree fingerprints,
  replacing the `DefDecl::source_hash` + fragment-file-hash invalidation
  with row-granular deps and retiring the remaining ambient body reads.
