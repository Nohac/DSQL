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

- **T2 — TS callsite contract: callsite ranges in metadata, not plugin
  detection.** Owner design direction (Jonas, 2026-07-13): the POC's
  Vite plugin re-detected `dsql(`…`)` callsites in app sources with its
  own scanning; instead, keep detection in the ONE place that already
  does it — the Rust embedding extractor — and ship the ranges through
  generate metadata. The plugin then becomes a dumb rewriter: look up
  this file's operations in the manifest, replace each recorded range
  with the generated operation import.

  The range needed is *not* the one we store today. Regions currently
  keep only the content span (between the backticks —
  `SourceOffset(content.start())` + the copied text); the rewrite needs
  the span of the entire `dsql(`…`)` expression. The extractor's regex
  full match (`captures.get(0)`) is exactly that and is currently
  discarded — record it as a fact on the region (e.g. `CallsiteSpan`)
  and flow it into `OperationMetadata` at assembly (optional: operations
  defined in plain `.dsql` files have no callsite).

  Open point for the T2 session — range freshness: metadata ranges are
  generate-time, but a Vite dev-server transform runs against the live
  buffer, which may have drifted since the last generate (the POC's
  plugin scanned live precisely because of this). Likely shape: emit
  the host file's content hash beside its callsite ranges; the plugin
  verifies the hash and on mismatch triggers/awaits a regenerate
  instead of rewriting wrong ranges. Build mode is unaffected (content
  is fixed). Interacts with what replaces the retired daemon protocol
  for watch-mode regeneration.

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

# Porridge issue: staleness on content revisit (A -> B -> A)

Restoring a document's earlier content — the save-undo-save flow —
leaves the resident bowl with wrong derivation state. Deterministic
repro: `checks::content_roundtrip_edits_rederive_cleanly` in
dsql-core (ignored until fixed). Bisected from the imdsql project down
to one document: two fragments where a length-changing edit to the first
shifts a second fragment carrying a reverse to-many relation selection
with a clause (`ratings: movie_info_idx(where .info_type_id == 101
order by id asc limit 1)`); apply the edit (compiles clean), restore the
original bytes (compiles clean when loaded fresh), and the settle ends
with `FieldNotFound` on the clause's order-by column.

Evidence gathered:

- The `ResolvedClause` facts are CORRECT post-settle (the order item at
  the reported span resolved, column present) — the stale party is the
  Complete-phase consumer, not the resolver.
- `explain` post-settle: `check_selections { matched_rows: 2,
  memoized_rows: 2, stale_views: 9 }` — fully memoized while its ambient
  views moved after its last run. A second settle does not heal it.
- An A -> B -> C sequence (different content each step) is fine; only
  revisiting a previously-held content hash breaks, in both edit
  directions (probe first or last fragment).
- A `DefIndex`-style tracked fingerprint over the resolution set does
  not close it: the aggregator itself runs early in the settle, reads
  the mid-settle view, commits a value equal to the previous
  generation's (hash-neutral, no revision bump), and never re-runs —
  even when driven per resolution row, explain still reports it
  memoized with a stale view while the checker's new tracked dep never
  moved. The engine's own memo/replan bookkeeping considers the settle
  converged while a Complete-phase consumer holds output computed from
  non-final views.

Affected surfaces: every ambient `View` consumer of Evaluate-phase
facts behind the Complete barrier — `check_selections`, `plan_queries`
(stale SQL is possible, not just stale diagnostics), `infer_variables`.
Fully tracked consumers (`lint_predicates`, `clause_tokens`) are fine.

Workarounds in place: the daemon full-reloads any batch whose file
revisits a content hash the resident bowl held earlier in its lifetime
(`Session::seen_hashes`; pinned by `plain_document_edits_roundtrip`).
Cost: an undo-style edit pays a project reload instead of an
incremental apply. The LSP path is NOT covered — editor undo can
strand the same stale state in a resident LSP bowl until the next
non-revisit edit or restart.
