# POC parity report — 2026-07-12

The feature/test-parity loop against dsql-poc is complete. Five commits
landed (`fd7c7bf..c8bd40f`), each phase plan and each implementation
reviewed by codex before commit (typically 2–4 review rounds; every
blocking finding was fixed before landing).

## Done

### Phase R — editor-service robustness (fd7c7bf, 7143780, 551fa58)

- **Marker-driven scenario harness** (`tests/it/scenarios.rs`): sources
  carry `<|>` cursor markers; each scenario snapshots the service
  answers (completion context + items + replace range, hover,
  definitions, diagnostics) at those positions. Bowl-level on purpose —
  transport concerns live in the dsql-lsp protocol harness
  (`dsql-lsp/tests/protocol.rs`, in-process duplex JSON-RPC with bounded
  waits).
- **Completion works everywhere, not just after a new relation** (the
  original complaint). The rewrite drives site classification and
  context-table resolution from one walk: the *open spine* of a
  cursor-truncated parse — the constructs still open at the cursor,
  stopping before anything that already has its closing token. Because
  the truncated parse ends exactly at the cursor, this classifies
  identically for well-formed and mid-edit sources; malformed sources
  (missing braces, dangling spreads, incomplete clauses) resolve their
  enclosing field's table where full-parse recovery loses the structure.
  Truncated-tree fields map onto resolver facts by span equality, so the
  semantic decision stays with the resolver.
- **Replacement spans**: `CompletionList` carries the identifier under
  the cursor; the LSP adapter emits `CompletionTextEdit`s, rebased into
  host coordinates for embedded regions (protocol-tested end to end).
  Mid-identifier positions answer identically from every cursor offset
  in the word (the identifier's start anchors the truncated parse).
- **Partial spreads**: `.`, `..`, `.Bi`, `..Bi`, `...Bi` all converge on
  a single `...Bits` — fragment items insert only the missing dots and
  the replace span covers the typed word; dotted positions suppress
  column/relation noise.
- **Grammar-keyword hygiene**: names-only sites (selection bodies, root
  selections, spread names, directive positions) take no grammar keyword
  items — the expected-token set at those frontiers is polluted by the
  previous construct's continuations.
- **Exhaustive range sweep**: `completion_context_sweeps_every_offset`
  requests completion at every byte of a representative document and
  pins the site/table/replace classification per offset range.
- Also: `map_cursor` region ends are inclusive (a cursor at a region's
  last byte still belongs to it), EOF and definition headers classify
  `DocumentRoot`/`Other` instead of over-offering tables, and a cursor
  after a nested `}` rebinds to the enclosing set.

### Phase S — CLI command parity (ed8e706)

New commands, all binary-tested via `CARGO_BIN_EXE_dsql` (13 end-to-end
tests pinning flags, stdout, and exit codes):

- `dsql init [path] [--database-url]` — scaffolds `dsql/dsql.toml` (a
  commented starter, URL escaped through facet_toml, parsed back through
  the real `Config` before touching disk) + `schema/`. Atomic
  (`create_new`) and fully rolled back on any failure; re-init refuses.
  With a URL it introspects immediately.
- `dsql validate` — everything generation does short of writing: prints
  all diagnostics, counts documents (ParsedFile entities: embedded
  regions in, hosts out) and queries (CST `QueryDef` count, so malformed
  and anonymous definitions still count), then dry-runs artifact
  assembly through the factored `dsql_generate::validate_assembly`
  (catches artifact-path collisions without creating `build/`). Fails
  only on error severity.
- `dsql check [file]` / `dsql fmt [file]` — optional path narrows to one
  document *within project context* (canonicalized against the project's
  own document set; outside files error). `check host.ts` shows region
  diagnostics projected to host coordinates; `fmt host.ts` refuses.
  `check`'s exit threshold is now errors-only (warnings/infos print
  without failing CI) — a deliberate improvement over both the POC
  (never failed) and our previous any-diagnostic behavior.
- `dsql parse <file>` — lossless CST + parse diagnostics, non-zero exit
  on parse errors.
- `dsql introspect --dry-run` — prints monolithic YAML, writes nothing
  (sink extracted and tested without a database).
- `dsql generate --target typescript-metadata [--out-dir]`,
  `dsql metadata-schema`, `dsql metadata-typescript` — the TS consumer
  contract (build-manifest JSON Schema + TS types); file output is
  byte-identical to the print commands. Mismatched flag/target
  combinations are rejected.

### Phase T1 — directive registry (c8bd40f)

- Static system registry: `@dsql.include_if` (fields; required
  `if:` boolean expression) and `@dsql.deprecated` (queries + fields;
  optional `reason:` string).
- The POC's six diagnostics with its precedence (unknown name stops; a
  misplaced directive still checks arguments; a duplicate argument
  skips its own unknown/type checks), locations derived from the parent
  entity's facts, shorthand `@.member` canonicalized to `dsql.member` in
  messages.
- Completions from the registry: `.` + `dsql` at `@`, members (with
  location details) after `@ns.`/`@.`, argument names with `name: `
  insert text after `(` and `,`, `true`/`false` (as keywords) only for
  boolean arguments after a written `:` — classification is structural
  (tokens, not frontier characters), so comments and missing colons
  cannot misclassify. No `null` leaks into value positions.
- **Deliberate deviation from the POC** (codex-flagged, needs your
  sign-off — also recorded in docs/issues.md): the POC *accepted*
  `@dsql.include_if` while its planner/metadata impls were empty, i.e.
  it generated silently-unconditional SQL. Here it validates fully and
  then still errors ("recognized but its semantics are not implemented
  yet") until the planner supports conditional selections.
  `@dsql.deprecated` is accepted (annotation with no downstream flow —
  exactly the POC's behavior).

### Test parity

142 tests across the workspace: 102 core integration tests (checks,
completions, scenarios, lowering, SQL, variables, scopes, embedding,
formatting, lints, scale), 13 CLI binary tests, protocol/LSP duplex
tests, project/init/introspection tests. All snapshot-based where the
POC was snapshot-based; the POC's LSP snapshot suite maps onto the
scenario harness (bowl-level) plus the protocol harness (transport).

## Kept out (and why)

- **T2 — TS callsite contract**: needs a design session with you before
  implementing; nothing was started. The generate-side contract surface
  (manifest schema/types) is in place, so the remaining work is the
  callsite shape itself.
- **`include_if` SQL semantics**: not in the POC either (empty planner
  impl); blocked on planner support for conditional selections. Until
  then the directive errors (see above).
- **`deprecated` metadata flow**: the POC never emitted it into
  artifacts; nothing to port. Worth deciding whether artifacts should
  carry deprecation info when T2 lands.
- **External directive definitions**: `register_external` was a dead
  hook in the POC (no callers). Omitted; the registry is a plain static
  table until something needs external registration.
- **Directive hover**: the POC had none.
- **`dsql lsp` / `dsql daemon` subcommands**: dsql-lsp ships as its own
  binary here; the POC's pre-LSP daemon protocol is retired.
- **Metadata schema-version validation on read**: `BuildManifest.version`
  exists in both codebases but nothing in the POC ever validated it on
  read; flagged as a potential improvement, not a parity gap.
- **Standalone single-file analysis**: the POC's `check <file>` spun up
  a standalone context with a hardcoded-catalog fallback for files
  outside any project. Ours requires a project and analyzes the file in
  its real resolution scope — strictly more correct, one analysis
  pathway.
- **Region-granular `fmt` for embedding hosts**: whole-file formatting
  is a dsql-document affair until region-granular edits are designed;
  hosts are skipped (and explicitly refused when named).

## Blocking

Nothing blocks day-to-day use. T2 is blocked on your design input.
One decision wants your explicit sign-off: the `include_if`
error-until-implemented disposition above.

## Porridge issues to solve

Recorded from this run, roughly by pain:

1. **`take()` livelocks are silent and brutal.** A live `QueryResult`
   held anywhere across `bind().take::<T>()` pins the entity's cells and
   take's blocked-loop spins at 100% CPU *forever* — no panic, no log.
   The same applies to `with_latest` under a held result. This cost
   hours (compounded by a tooling no-op, but the engine gave zero
   signal). Wanted: a deadlock diagnostic (detect same-task pinned cells
   and panic with the holder's location), or documentation + a debug
   assertion.
2. **`take()` bumps the entity and reaps derived sibling stamps.**
   Scooping `CompletionContext` must happen *before* taking
   `CompletionList` from the same request entity. Workable once known,
   but the ordering constraint is invisible in the API. A `take` variant
   that doesn't bump, or a way to take several components atomically,
   would remove the trap.
3. **No external whole-entity despawn.** Request entities are cleaned up
   via take-bumps; anything else lingers. Adapters want a real despawn.
4. **The same-phase View/commit conformance panic is excellent** — it
   caught a real race in the directive checks immediately and named both
   sides. Keep it; more of this.
5. **Deferred designs already filed in docs/issues.md** (owner input
   wanted): source residency & representation (eviction, region facts
   instead of copied ropes), per-file diagnostics demand (debounce),
   and walk decomposition (row-granular deps replacing
   `DefDecl::source_hash` + fragment-file fingerprints — this is what
   recovers fragment-file keystrokes from ~52–60ms to the ~32ms other
   files get).
6. **Cold-start cost** stands at ~120–150ms debug for `check` on the
   reference project vs the POC's ~80–100ms (accepted earlier; the gap
   is bowl assembly, not language work).
