# Architecture, correctness, and test review — 2026-09-06

DSQL is a capable alpha with a coherent compiler architecture and unusually
substantial incremental-behavior coverage. The next milestone should be
correctness and integration hardening. The evidence does not justify another
compiler rewrite, but it also does not justify calling the current boundaries
production-hardened.

Reviewed at `b0254a691ce19da28498870e3172c489ff90d578`, including the existing
working-tree edits to the Observatory queries. No implementation or existing
snapshot was changed. This report is review evidence; `docs/plan.md`,
`docs/architecture/compiler.md`, and the specs retain their authority. The older
untracked `docs/codebase-review.md` was preserved and its findings were not
assumed to describe today's implementation. No Opus/Fable review was used.

## Scope and verification

The review covered all nine first-party crates, TypeScript runtime/rendering/
daemon/Vite integration, editor grammar tooling, the Observatory fixture,
shared conformance cases, current architecture documents, and the issue backlog.
The test review covered every first-party test harness, its coverage areas,
representative assertions and snapshots, and shared helpers. Particular scrutiny
went to cross-layer contracts, incremental ownership, publication, and failure
paths. This is a broad source and executable review, not a claim that every line
of every snapshot or every possible feature combination was exhaustively proved.
Vendored code was reviewed at the local patch ledger and consumer boundaries;
the complete upstream Logos and Lelwel implementations were not independently
re-audited.

The Rust tree contains 123 `.rs` files and approximately 69,800 lines, including
42 test-source files with approximately 23,000 lines. There are 278 committed
snapshot files. These are inventory counts, not measures of coverage quality.

| Check | Result |
| --- | --- |
| `cargo clippy --workspace --all-targets` | Passed without warnings |
| `cargo fmt --all -- --check` | Passed |
| `cargo test --workspace --all-targets` | 456 passed; 3 ignored |
| Three ignored project probes, run explicitly | All passed; the two stall probes completed 200 sequential and 100 concurrent rounds |
| `cargo test --workspace --doc` | Passed; no executable doctests |
| Storage-independent generation on `wasm32-unknown-unknown` | Passed with `--no-default-features --lib` |
| TypeScript `bun run typecheck` | Passed |
| TypeScript suite with `DSQL_BIN=../../target/debug/dsql` | 107 passed, including the real daemon |
| Editor surface, captures, corpus, fixture checks | Passed: 57 literals, 5 terminals, 4 corpus cases, 59 highlight assertions, 17 fixtures |
| Neovim indentation oracle | Passed |
| Database-gated execution, introspection, CLI tests | All 7 passed on disposable PostgreSQL 18.4 with the correctness seed |

The default Rust count includes seven database tests that return early when
their environment variables are absent. They were subsequently exercised
against a separate temporary database, which was stopped and removed. This was
PostgreSQL 18.4, not the Observatory's configured default 17.5 image. The current
working-tree Observatory queries passed that live run.

Initial Bun child-process tests timed out inside the sandbox. An isolated case
passed outside it, followed by the complete 107-test suite. Neovim also required
running outside the sandbox. Those failures are environment limitations, not
reported product defects. The small edit-cost probe completed at roughly
2–6 ms per edit in this debug run; that is not a large-project performance claim.

## Reproduced defects

### 1. High: ordinary SQL variants can rewrite user literals

**Evidence:** `crates/dsql-core/src/sql/postgres.rs:248` and `:509`,
`crates/dsql-execute/src/lib.rs:229`, and the TypeScript variant materializer.
Variants use public path strings as placeholders and global text replacement.
Numeric sentinels and bounded dynamic sites have collision handling; ordinary
operator/direction variants do not.

Against the IMDb project fixture:

```dsql
query ReviewVariant {
  title(where .title == "{{params.direction}}" order by id %direction) {
    id
  }
}
```

Running `dsql operation execute ReviewVariant --scope default --dry-run
--variables '{"params":{"direction":"desc"}}'` produces a predicate comparing
the title to `'desc'`. It must continue comparing to the literal
`'{{params.direction}}'`. Both compilation and materialization report success.

This is silent query-semantic corruption, not an SQL injection demonstration.
Existing numeric/dynamic-marker regression tests do not cover this variant
family. Represent replacement sites structurally, or allocate collision-checked
opaque markers for every variant kind. Add compiler-to-materializer cases in
both runtimes, including literals resembling formatted placeholders and repeated
static/dynamic ordering of the same column.

### 2. High: catalog publication treats database names as filesystem paths

**Evidence:** `crates/dsql-project/src/metadata.rs:215`. Schema names are joined
directly onto the staging path; table names are suffixed with `.yaml` and joined
without confinement checks.

A temporary probe loaded fixture metadata, renamed a schema and its tables'
schema fields to `../escaped`, and removed unrelated foreign keys. The metadata
passed `to_catalog()`. Publishing it succeeded, wrote `escaped/title.yaml`
outside the staged/live schema directory, and subsequently reloaded zero schemas
from the successfully published directory.

This can overwrite YAML outside the publication root, as well as lose catalog
contents. Database identifiers are not safe path components. Encode names into
reversible filenames or explicitly reject unsupported names before any write;
apply the same mapping to loading and catalog navigation. Test schema and table
names containing separators, parent components, and absolute paths entirely in
temporary directories.

### 3. High: catalog readers can observe mixed publication generations

**Evidence:** `crates/dsql-project/src/metadata.rs:44`, `:109`, and `:185`.
Publication stages files, renames the live directory to a fixed backup, then
promotes staging. Readers independently open table paths under the live name.
Neither readers nor writers share a lock, and a reader can attempt recovery
during an active writer's rename window.

An isolated concurrent probe published 20 revisions of a 20-table catalog while
loading it repeatedly. One run produced 16 successful reads, of which 15 mixed
table descriptions from different generations, plus 7 failed reads. All 20
publications reported success. Exact counts are schedule-dependent; a single
mixed read violates the claimed complete-snapshot boundary.

The staged-failure and rollback tests are useful, but do not establish concurrent
read atomicity. Use immutable generation directories with one captured read
pointer, or hold an appropriate shared/exclusive lock for the entire load/store
transaction. Recovery must be coordinated with writers. Add a concurrent
reader/writer test that asserts every returned catalog belongs to one generation,
plus writer contention and interrupted-promotion cases. Artifact publication's
separate advisory-lock implementation does not protect catalog publication.

### 4. Medium: valid JSON with an invalid daemon response shape crashes Node

**Evidence:** `integrations/typescript/src/daemon.ts:422`. `JSON.parse` is cast to
a response type, then `response.id` is accessed outside the parsing catch.

A fake child that answers the first request with `null\n` caused Node to exit
with `TypeError: Cannot read properties of null (reading 'id')`. The caller's
`try/catch` around `await client.compile()` never received a normal session
rejection. Existing tests cover syntactically invalid JSON and wrong IDs, but
not this valid-JSON case.

Validate the envelope as an object before accessing it, require the intended
result/error shape, and validate handshake fields before caching them. Route
all protocol-shape failures through the existing session failure path. Test
`null`, arrays, missing outcomes, malformed errors, and incompatible handshake
responses. The Rust protocol parser already takes a substantially stricter
approach than this consumer.

### 5. Medium: warm and cold document assignment rules disagree

**Evidence:** `crates/dsql-core/src/source.rs:388` deduplicates equal scope/resolver
assignments. `crates/dsql-project/src/documents.rs:115` rejects a file encountered
in a second document group, even when that group has the same scope and resolver.
Daemon relevance classification uses the former rule.

Reproduction: configure one scope with two identical document groups selecting
`queries/*.dsql`, initially with no matching file. Initialize and compile the
daemon, then create `queries/new.dsql`. `filesChanged` succeeds and publishes the
operation; an immediate full `compile` returns `ProjectLoadFailed` for duplicate
assignment. The same project contents have different validity depending on how
they entered the compiler.

Choose one duplicate-group rule and enforce it through shared ownership logic.
Preserve assignment provenance if duplicates are errors. Extend the existing
warm/cold and ambiguity tests to overlapping groups within the same scope and
resolver, not only conflicts between distinct assignments.

### 6. Medium: Rust and TypeScript accept different scalar input languages

**Evidence:** `crates/dsql-execute/src/lib.rs:838` and `:1219`;
`integrations/typescript/src/runtime.ts:798`, `:834`, and `:1133`.

Direct materialization probes established these differences:

| Input | Rust | TypeScript |
| --- | --- | --- |
| Timestamp `2024-01-01t00:00:00z` | Accepts | Rejects |
| Timestamp `2016-12-31T23:59:60Z` | Accepts | Rejects |
| Integer JSON token `5` | Accepts | Accepts |
| Integer JSON tokens `5.0` and `5e0` | Rejects | Accepts after JSON parsing |

The timestamp implementation combines Chrono parsing in Rust with a handwritten
regex/calendar validator in TypeScript. Rust's integer helper preserves the
JSON parser's integer/float category; JavaScript does not preserve that lexical
distinction. The Rust unit test explicitly pins the category distinction, so
this needs a contract decision rather than an arbitrary parser substitution.

Specify the accepted wire language and extend the shared conformance corpus
with these boundary cases. For lexical-number requirements, retain raw JSON
tokens in the conformance harness; otherwise a JavaScript parse/serialize cycle
erases the distinction under test. Keep language-specific implementations behind
the same observable contract. No choice about whether leap seconds should be
accepted is implied by this review.

### 7. Medium: introspection omits partitioned table parents

**Evidence:** `crates/dsql-introspection/src/lib.rs:110` restricts the column
query to `relkind IN ('r', 'v', 'm')`.

In the temporary PostgreSQL database, creating `partitioned_events PARTITION BY
RANGE(id)` and one partition, then running successful CLI introspection, wrote
`partitioned_events_0.yaml` but no `partitioned_events.yaml`. The logical parent
is unavailable to DSQL while its physical partition is exposed.

Support partitioned parents deliberately, including their constraints and type
closure, or reject them with a clear unsupported-object diagnostic. Add a live
fixture for a parent, child, and foreign key referencing the parent. The current
live fixture is rich in types, views, indexes, and composite keys but does not
exercise this object category.

## Architectural risks and code smells

These are source-supported concerns or already-tracked limitations, distinguished
from the reproductions above.

**LSP revision consistency remains incomplete.** In
`crates/dsql-lsp/src/server.rs:171`, the rope, diagnostics, and host projection
come from separate awaits. Completion and semantic-token handlers similarly
obtain a rope and then request derived results without one revision boundary.
Edits can occur between those reads. The publisher mutex orders publication
handlers, but does not freeze source revisions; `did_close` also calls
`publish_diagnostics` outside that mutex. Document versions are not retained or
sent with diagnostics. Formatting already recognizes the issue and holds the
mutation lock while capturing its inputs. Apply an equivalent coherent snapshot
contract to other responses, release locks before client I/O, and discard stale
work. This interleaving was identified from code, not forced by a deterministic
test in this review.

**Editor disk/config lifecycle is unfinished.** The LSP loads the project during
initialization and does not implement project/schema watcher reconciliation.
Closing an unreadable or deleted file retains its last buffer text. This can
retain definitions for a file that no longer exists. Test close-after-delete,
new unsaved files, external provider edits, and catalog/config reloads. Passing
did-close restoration for an existing readable file does not cover these states.

**Fragment expansion is shared, but can still expand exponentially.**
`ExpansionOccurrence` represents each simple spread path and `ExpansionBody`
clones its semantic payload. Checks, variable inference, and planning then build
their own `DefinitionClosure` views. This is much better than independent
resolution decisions, but sharing semantics has not eliminated repeated data
materialization. The depth-eight case already records 511 occurrences for one
query and 1,515 across roots. The open issue
[`dab86061`](../issues/dab86061-bound-fragment-expansion-occurrence-growth.md)
accurately identifies this risk. Add a deterministic per-root resource budget
before treating arbitrary project input as cheap to process; measure allocation
and occurrence counts as well as system invocations.

**Ownership correctness demands substantial engine-specific knowledge.** Narrow
projection anchors, untracked inverse relationships, producer-owned site roots,
and Complete-phase barriers are deliberate and well documented. However, a new
contributor must understand all of them to make an apparently local semantic
change safely. The pending
[`definition-site retirement check`](../issues/cbc3459d-verify-definition-site-retirement-after-in-place-definition-deletion.md)
should be closed with direct in-place deletion/restoration evidence. It is an
unverified lifecycle question, not a confirmed stale-site bug. A reusable
warm-versus-cold edit-sequence harness would protect this architecture better
than another layer of registration abstractions.

**Large modules increasingly combine distinct responsibilities.** `policy.rs`
has roughly 3,600 lines covering syntax, visibility, compilation, diagnostics,
and editor services. `plan/build.rs` is about 3,000 lines; `variable.rs` and
`sql/postgres.rs` about 2,300 each. Execution and introspection put substantial
algorithms in `lib.rs`, contrary to CONTRIBUTING's slim-entrypoint rule. Keep
vertical language ownership, but give policy compilation, visibility projection,
and service presentation named internal modules. Split execution's materializer
from PostgreSQL binding. Do this alongside behavior changes with existing
integration coverage; a line-count-driven mass split would distribute complexity
without reducing it.

**Boundary knowledge is still duplicated.** Ordinary/dynamic scalar validators
exist in both runtimes, document ownership has cold/warm variants, host-generator
launch logic appears in native generation and daemon execution, and catalog versus
artifact publication have different transactional guarantees. These are more
valuable DRY targets than repeated Rust tuple declarations. The shared metadata
types and conformance corpus are already the right starting points.

**Some small cleanup rules are unenforced.** Production Rust is notably restrained
about panics and unsafe code. The scan found one `unreachable!()` in daemon
dispatch, one `#[allow(clippy::too_many_arguments)]` in the SQL renderer, and one
unsafe slice conversion in CLI rendering with a SAFETY explanation. Many public
metadata fields and several public functions lack the documentation requested by
CONTRIBUTING. Treat these as local maintenance work, not comparable in severity
to the reproduced correctness defects.

## What the tests establish, and what they miss

| Area | Strong evidence today | Most useful next coverage |
| --- | --- | --- |
| Parse/lowering/definitions/selections | Grammar ownership, recovery spans, normalized facts, contextual names, cardinality | Generated malformed-input and edit sequences; parser/formatter semantic preservation |
| Scopes/contexts/policies/expansion | Visibility collisions, cycle cuts, trusted context, provider movement, deletion/reinsertion, warm/cold comparisons | Same-assignment overlap; definition-site retirement; occurrence budget; additional empty-set transitions |
| Variables and effective input contracts | Defaults, exact refinements, containment/lifting, context propagation, cycles, invalid bindings | Wire-language parity at lexical/numeric/timestamp boundaries |
| Planning/SQL/aggregates | Detailed SQL snapshots, optional predicates, policy masking, flattening, multi-root operations, fragment edits | Ordinary variant collisions; execute representative generated combinations rather than only snapshot SQL |
| Core services/formatting | Every-offset completion sweeps, malformed documents, host projection, comments, idempotence | Rename/navigation coverage across every semantic target; formatting preserves parsed meaning as well as layout |
| Project/catalog/overlays | Strict overlay conflicts, provider type identity, canonical fingerprints, staged rollback | Concurrent readers/writers; unsafe names; symlink/discovery rules; partitioned objects |
| Generation/publication | Pre-write diagnostics, artifact collisions, immutable generation IDs, locking, retention, match-lock behavior | Failure injection around every publication boundary; explicit read consistency contracts |
| Native execution/introspection/CLI | Shared input corpus, live binds, real SQL, policies, composite relations, dry-run behavior | More database object categories; one required database gate with explicit skip accounting |
| Daemon/LSP protocols | FIFO, EOF, failed reload recovery, replay, diagnostics, UTF-16 mapping, semantic tokens, formatting | Malformed response shapes; revision-consistent LSP responses; deleted files and slow-client interleavings |
| TypeScript runtime/rendering/rewrite/Vite | Real daemon round trip, Node ESM loading, input codecs, sparse result parsing, HMR recovery, byte offsets | Real framework build/runtime fixture; schema-shaped protocol validation; syntax-aware extraction cases |
| Editor integration | Surface drift, capture standards, corpus, fixtures, actual Neovim indentation | Broader invalid/recovery corpus and systematic parity with compiler syntax |

Several test-design details deserve attention:

1. **Default green does not mean live SQL ran.** Seven Rust tests return normally
   when a database variable is absent, so Cargo labels them passed. There is no
   checked-in CI workflow in this checkout that makes database setup and these
   gates mandatory. Keep fast local testing, but provide a required integration
   command/job that fails when its database is unavailable and reports which
   optional gates ran. Likewise, ordinary Bun testing skips the real daemon
   unless `DSQL_BIN` is set.
2. **Some snapshots omit diagnostic identity/provenance.**
   `render_diagnostic_facts` in core test support renders severity, span, and
   message but not file, code, or stage. Moving a diagnostic to the wrong file
   at the same offsets can be invisible there. Dedicated context/policy tests
   inspect more information; make a full stable diagnostic rendering reusable.
3. **Zero-work assertions can pass without observing a system.** `runs_of` in
   `tests/it/scale.rs` returns zero when a suffix does not match any registered
   system. Renaming/removing a system can silently preserve tests expecting a
   zero delta. Require exactly one matching counter and assert the initial
   producer ran where relevant. Keep the unrelated-owner isolation tests; they
   are valuable. Exact convergence counts should be separated from asymptotic
   work bounds so harmless scheduler improvements are easy to assess.
4. **The concurrent stall probe discards task failures.**
   `hang_repro.rs` awaits the open and hover join handles using `let _ = ...`.
   It verifies completion before timeout, not that both tasks and the hover
   response succeeded. Assert the nested results as well. The explicit run here
   passed, but this weakness limits what that green result proves.
5. **Framework tests use substitutes.** The TanStack renderer test asserts many
   generated substrings and typechecks a consumer against stub React Query and
   server-function declarations. The source templates carry `@ts-nocheck`, which
   the generator correctly strips from emitted files; checking the package alone
   therefore does not check those template bodies. These tests prove useful
   generation properties, but not compatibility with a real
   installed TanStack application or its server/client bundler boundary. Add one
   small actual framework fixture and inspect its browser bundle for SQL and
   server-only dependencies.
6. **Big scenario tests hide breadth behind small counts.** The single live CLI
   test exercises many operations, while a first failure prevents later cases
   from running. Split independently set-up behaviors into named cases while
   sharing database setup. Preserve the common JSON conformance cases; they are
   more valuable than separately authored runtime expectations.
7. **Formatting idempotence and SQL snapshots are necessary but insufficient.**
   An idempotent formatter can delete meaning, and wrong SQL can have a stable
   snapshot. Pair these with token/semantic equivalence and selected live result
   assertions. Broader property-based or fuzz testing is not currently represented
   in the repository's test setup.

## Where the project stands

The architecture has successfully progressed beyond the port skeleton. Its
strongest decisions should remain:

- One effective catalog boundary, with provider metadata and authored overlays
  composed before the compiler sees them.
- Generated parser/lexer ownership, exhaustive lowering/formatting dispatch,
  and a small syntax-stage contract.
- Resolved semantic facts reused across diagnostics, services, inference, and
  planning, with name- and owner-granular invalidation tests.
- One effective input contract and checked fragment rewriting decision rather
  than independent inference in each consumer.
- Pure in-memory generation separated from native loading and publication;
  the actual WASM check passes.
- A real end-to-end path from project discovery through checked SQL and metadata
  to Rust execution and TypeScript/Vite integration.
- Strict manifest versioning, a pinned Bowl revision, substantive failure-path
  tests, and an explicit vendor patch ledger.

Queries, fragments, aggregates, scoped predicates, defaults/lifting, trusted
contexts, policies, overlays, provider types, native enums, daemon generation,
LSP services, and host code generation are implemented. Mutations, split fetch,
and broader computed/function/pipeline features remain design work; their absence
is not a regression. Checked directive syntax exists, but external
generator-visible directive contracts remain an empty projection in
`dsql-generate/src/contract.rs`.

This is best described as **a feature-rich, locally validated alpha suitable for
continued dogfooding in controlled projects**. It has stronger compiler semantics
and regression coverage than its packaging and operational boundaries. The
TypeScript package is still private/experimental, the checkout has no root
README, and no checked-in CI pipeline demonstrates the full set of required
checks. There is no evidence here for broad production scale or compatibility
across a supported OS/database/framework matrix.

Documentation is much improved over the earlier review, but still contains
contradictions. The current compiler document describes direct-import spread
visibility where code/tests implement transitive visibility, and says full text
materializes only at parsing while daemon hashing also materializes it. The
fact-marker commentary promises idle-driven diagnostics while LSP initialization
arms global demands and edits publish immediately. Bring these exact statements
in line with current behavior; do not reopen the historical port checklist as if
the implemented phases were missing.

Using the supplied skills as qualitative rubrics: **software-design complexity
is about 6/10**. Crate boundaries are clear and mostly deep; the weak diagnostic
rows are describing large modules in one sentence, changing implementations
without coordinated consumer edits, and newcomers understanding lifecycle
contracts without reading several implementations. Named internal responsibilities,
shared boundary contracts, and lifecycle test helpers address those gaps.
The claimed percentage of time spent on design cannot be inferred from source.

For **Pragmatic Programmer's repository-observable questions, about 7.5/10**:
adapters isolate database access, the end-to-end path exists, and the baseline
code is clean; single ownership of every behavioral rule fails at the boundaries
documented above. Centralize those decisions and prove the remaining duplicated
runtime implementations against one corpus. Estimation habits, weekly learning,
and five-minute deployment rollback are not established by this checkout and
are excluded from that score. Neither score is a completion percentage.

## Recommended next work

1. **Close the reproduced boundary defects before adding language surface.**
   First fix catalog path confinement and catalog snapshot consistency, then SQL
   variant substitution. Follow with daemon response validation, shared document
   assignment rules, wire-input parity, and partitioned-table handling. Each is
   a coherent fix with a focused regression; avoid bundling them into one large
   refactor.
2. **Turn the current checks into an explicit acceptance gate.** Require the
   workspace checks, WASM boundary, generated metadata freshness, TypeScript
   typechecking/real daemon, editor gates, and a seeded live database run. Make
   missing prerequisites visible. Add the actual framework fixture before
   broadening the integration's support claims.
3. **Finish incremental and editor lifecycle hardening.** Bound fragment
   occurrence growth, prove definition-site retirement, and add revision-consistent
   LSP snapshots plus disk/config reconciliation. Build a reusable edit-sequence
   harness comparing warm output to a fresh bowl, including malformed edits,
   moves, deletions, reinsertions, and empty sets.
4. **Reduce complexity where these fixes reveal shared knowledge.** Refactor
   policy internals and the execution entrypoint locally. Measure closure cloning,
   allocation, source hashing, and definitions-times-filters work before optimizing
   individual `.clone()` calls. Keep the shared semantic fact model.
5. **Then choose one product milestone and finish it end to end.** A documented
   PostgreSQL + TypeScript application with reliable editor feedback, reproducible
   generation, and a tested server/client boundary would provide more confidence
   than implementing several deferred language specs at once.

Of the eight currently open issue files, occurrence growth and definition-site
retirement deserve the earliest attention. Navigation unification, input-role
provenance, refinement completion, numeric bigint defaults, static predicate
folding, and source-digest reuse are reasonable subsequent work. The newly
reproduced correctness defects above should take precedence over those polish
and optimization items.
