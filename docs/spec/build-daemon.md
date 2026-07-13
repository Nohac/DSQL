# Build Daemon & Host-Tool Integration

Status: draft v5 (2026-07-13, revised after four review rounds).

The build daemon is a persistent compile service that host build tools —
Vite, other bundlers, file watchers, task runners — drive over stdio to
compile a dsql project and receive generated artifacts, callsite ranges,
and diagnostics. Consumers are deliberately thin: detection, analysis,
and **compiler artifact generation** live in the Rust tooling; a
consumer transports requests, runs its **host-language renderer** (the
one deliberately consumer-side piece — it turns artifacts into
host-language modules), splices precomputed ranges, and presents
diagnostics. Vite is one binding among potentially many; nothing in
this spec is Vite-specific.

Lessons from the proof of concept, which this spec supersedes:

- The POC daemon (`compileProject` + `shutdown`) kept a warm *process* but
  cold *analysis* — every hot update re-analyzed the whole project from
  disk. This daemon holds the project bowl resident and applies file
  changes incrementally (`filesChanged`), which is where the engine's
  delta planning actually pays off (~32ms per changed file vs a
  ~120–150ms cold reload on the reference project).
- The POC's Vite plugin re-detected `` dsql(`…`) `` callsites in app
  sources with its own scanning. Here, callsite ranges ship in the
  compile response (single source of truth: the embedding extractor),
  and the consumer rewrites blindly — see Callsites and freshness.
- The POC had no protocol handshake, versioning, error codes, or defined
  lifecycle. This protocol specifies all four.

One daemon serves one consumer: each binding spawns and owns its own
child process. A shared multi-consumer daemon is out of scope for
version 1, but concurrent *publication* is guarded regardless — see the
publication lock under Transactionality.

## Transport and framing

Line-delimited JSON over the daemon's stdin/stdout, spawned as
`dsql daemon`. Each request is exactly one UTF-8 encoded JSON object on
one line; each response likewise. Blank lines are ignored. stderr is
reserved for human-readable logging and MUST NOT carry protocol data.

```json
{"id": 1, "method": "initialize", "params": {"protocolVersion": 1, "root": "/abs/project"}}
{"id": 1, "result": {"protocolVersion": 1, "projectBase": "/abs/project", "configPath": "dsql/dsql.toml", "schemaDir": "dsql/schema", "buildDir": "dsql/build", "generatorOutputs": []}}
```

- `id` is a consumer-chosen integer in `1..=2^53-1`, echoed verbatim.
  Consumers SHOULD keep in-flight ids unique; the daemon does not
  deduplicate.
- Every response carries exactly one of `result` or `error`.
- **Execution is strictly sequential**: the daemon processes requests
  one at a time in receive order and responds in the same order.
  Consumers MAY pipeline requests; they MUST NOT assume concurrency.
  (The resident bowl is mutable state; serialization is the contract,
  not an implementation detail.)
- A line that does not parse as a request object is answered with
  `{"id": null, "error": {"code": "InvalidRequest", …}}` — never `id: 0`,
  which could collide with a real request. Malformed lines do not poison
  the session.
- The daemon never initiates messages. Server-initiated events (e.g.
  push diagnostics from daemon-side watching) are out of scope for
  version 1; consumers own file watching.

### Errors

Every `error` carries a stable machine-readable `code`, a human
`message`, and the code's `data` payload:

| code                         | meaning / `data`                                                                     |
| ---------------------------- | ------------------------------------------------------------------------------------ |
| `InvalidRequest`             | unparseable line, unknown method, bad params. `data: { method?: string }`             |
| `NotInitialized`             | request before `initialize`. `data: null`                                             |
| `AlreadyInitialized`         | second `initialize`. `data: null`                                                     |
| `UnsupportedProtocolVersion` | version mismatch; daemon stays uninitialized. `data: { daemonVersion: int }`          |
| `InvalidPath`                | request path lexically outside the project. `data: { path: string }`                  |
| `ProjectLoadFailed`          | config/schema/catalog failed to load. `data: { path?: string, message: string }`      |
| `Diagnostics`                | compile failed on language errors. `data: { diagnostics: Diagnostic[] }` (see below)  |
| `ArtifactCollision`          | build-path collision. `data: { kind, first, firstSource, second, secondSource, path }`|
| `AssemblyFailed`             | metadata assembly/serialization failure. `data: { artifact: string, message: string }`|
| `PublicationLocked`          | another process holds the publication lock past the wait bound. `data: null`          |
| `Io`                         | file system failure. `data: { path: string }`                                         |
| `GeneratorFailed`            | host generator command failed **after** commit. `data: { generationId, manifestPath }`|
| `Internal`                   | daemon bug; consumers should restart. `data: null`                                    |

The `Diagnostic` shape (shared by the `Diagnostics` error and the
success-path `diagnostics` snapshot):

```jsonc
{
  "file": "src/components/TitlePanel.ts",
  "range": { "start": 120, "end": 128 },       // host coordinates
  "embeddedRange": { "start": 66, "end": 74 }, // region coordinates, when embedded
  "severity": "Error",                          // Error | Warning | Info
  "source": "Check",                            // Parse | Lower | Check | Lint | Plan | Generate
  "code": "UnknownColumn",
  "message": "column `bogus` does not exist on `public.title`"
}
```

## Lifecycle

The daemon is a three-state machine: `Uninitialized → Ready →
ShuttingDown`.

- Any request other than `initialize`/`shutdown` while `Uninitialized`
  is a `NotInitialized` error. A failed `initialize` (bad version,
  missing project) leaves the daemon `Uninitialized`; the consumer may
  retry with corrected params.
- A second `initialize` while `Ready` is `AlreadyInitialized`.
- `shutdown` is valid in any state.
- **EOF on stdin**: the daemon stops reading, **finishes the in-flight
  request to its transactional end** (publication and host generator
  included — no torn build trees), skips the response, and exits. The
  consumer's kill-after-timeout is the escape hatch for a hung
  generator; an abruptly killed consumer never leaks a daemon.
- Consumer obligations on the other side: reject all pending requests
  when stdout closes or the child exits; apply a timeout to `shutdown`
  and then kill the child; bound accumulated stderr; on unexpected
  daemon death, discard ALL cached state (compile results, ranges,
  render maps), restart with backoff, and re-run a full `compile`
  before transforming anything.

## Methods

### `initialize`

Params: `protocolVersion` (integer), `root` (absolute path to discover
the project from — the daemon walks up to `dsql/dsql.toml` like every
other entry point), and optional `excludeRoots` (project-base-relative
directories the consumer's renderer owns — its `ownedRoots` — validated
by the same rules as generator outputs below).

Must be the first request. Version negotiation is **exact equality** in
v1: any mismatch answers `UnsupportedProtocolVersion` and the daemon
stays uninitialized.

The result returns the daemon's `protocolVersion` and the canonical
paths every later exchange is relative to: `projectBase` (absolute,
canonicalized once here — symlinks are resolved at this boundary and
never again), the project-base-relative `configPath`, `schemaDir`, and
`buildDir`, plus `generatorOutputs`: the project-base-relative output
directories the project's configured host generator command declares
(see Host generator command below) — the consumer excludes these from
watching alongside `buildDir`.

Path rules for the whole protocol: relative paths use `/` separators
and are relative to `projectBase`. Containment ("inside the project")
is judged **lexically** on the normalized path string; symlink targets
are deliberately not policed — a project's own tree is trusted, and
re-resolving symlinks per request would make path identity unstable.

### `compile`

Params: none (the root is fixed at `initialize`).

Full compile: (re)load the project from disk, settle, assemble, publish
(see Transactionality), and answer with the compile result below.

### `filesChanged`

Params: `paths` (array of paths, absolute or project-base-relative;
normalized lexically against `projectBase` — no symlink resolution).

Incremental compile: each path is examined on disk and applied to the
resident bowl —

- an existing **file** is re-read and applied as a source edit
  (documents and embedding hosts alike — region re-extraction is the
  engine's job); a file new to the project loads into its configured
  scope;
- an existing **directory** is rescanned and its subtree reconciled
  (files added/updated/removed to match disk) — this is how bulk
  operations and directory-level watcher events stay expressible;
- a **missing** path removes the file, or every project file under that
  prefix (a deleted directory needs no type discovery);
- a path lexically outside the project is `InvalidPath`; a path inside
  the project that matches no configured scope is silently ignored (the
  daemon, not the consumer, knows what is relevant).

Renames arrive as two events (old path deleted, new path created).

**No-op batches**: if, after reconciliation, nothing relevant changed
(every path irrelevant, or contents identical), the daemon replays its
**latest compile outcome** — whatever it was — without settling,
publishing, or running generators. Saving `README.md` must not rerender
the world, and it must not *un-report* anything either:

- last outcome was a success → repeat the result with the same
  `generationId` and `"changed": false`;
- last outcome was `Diagnostics` → repeat that error and its snapshot
  (the bowl is still invalid; a clean earlier result must not resurface
  and wipe the binding's error display);
- last outcome was any other error (`GeneratorFailed`,
  `ArtifactCollision`, `AssemblyFailed`, `Io`, `PublicationLocked`) →
  repeat it verbatim; nothing is retried on a no-op — `compile` is the
  explicit retry;
- project load is still failing → retry the full load as specified
  above (the one exception, because a config fix arrives as exactly
  such a change).

Otherwise the result, publication, and error behavior are identical to
`compile`. A `filesChanged` arriving **before the resident bowl has
been loaded** (i.e. before any compile attempt got as far as loading —
a compile that failed with `Diagnostics` counts as loaded) is
equivalent to `compile`.

Changes to the project config (`configPath`) or anything under
`schemaDir` invalidate resident state: the daemon performs a full
reload behind the same request/response. **If that reload itself fails**
(broken `dsql.toml`, bad schema), the resident bowl is discarded, the
request answers `ProjectLoadFailed`, the last published build tree
stays untouched, and every subsequent `compile`/`filesChanged` retries
the full load until it succeeds.

**Failure retention**: a compile that fails with `Diagnostics` keeps the
updated (invalid) bowl resident and leaves the last successfully
published build tree untouched on disk. A later `filesChanged` repairs
the state incrementally; no restart or full recompile is required.

### `shutdown`

Params: none. Answers `{"result": true}` and exits.

## Compile result

The `result` of `compile`/`filesChanged`:

```jsonc
{
  "generationId": 7,                       // project-monotonic (see Transactionality)
  "changed": true,                          // false only on no-op batches
  "manifestPath": "dsql/build/manifest.7.json",       // immutable, matches this result
  "currentManifestPath": "dsql/build/manifest.json",  // the fixed pointer
  "manifest": { /* BuildManifest, see metadata schema */ },
  "artifacts": [
    {
      "id": "frontend/operation/TitlePanel", // stable: scope/kind/name
      "kind": "operation",                   // operation | fragment
      "scope": "frontend",
      "metadata": { /* OperationMetadata | FragmentMetadata */ }
    }
  ],
  "groups": [
    {
      "name": "frontend",
      "imports": ["shared"],
      "artifacts": ["frontend/operation/TitlePanel", "shared/fragment/TitleBits"]
    }
  ],
  "sourceFileScopes": [
    { "path": "src/components/TitlePanel.ts", "scope": "frontend" }
  ],
  "callsites": [
    {
      "path": "src/components/TitlePanel.ts",
      "contentHash": { "algorithm": "sha256", "value": "…lowercase hex…" },
      "expressions": [
        {
          "range": { "start": 54, "end": 213 },   // the whole dsql(`…`)
          "definitions": [
            { "kind": "query", "name": "TitlePanel", "id": "frontend/operation/TitlePanel" }
          ]
        }
      ]
    }
  ],
  "diagnostics": [ /* Diagnostic[], complete snapshot */ ]
}
```

- **Artifacts are stored once** in the flat `artifacts` array; `groups`
  reference them by `id`. Ids are stable, opaque strings (currently
  `scope/kind/name`) and remain unique when independent scopes are later
  allowed to reuse names — consumers key on `id`, never on bare `name`.
  `groups[].artifacts` is normatively the group's **effective resolution
  closure**: the scope's own artifacts plus everything visible through
  its imports (the example's imported `TitleBits` is included by rule,
  not by accident).
- **Ordering is stable and normative** for reproducibility: `artifacts`
  by `id`, `groups` by `name`, `callsites` by `path`, `expressions` by
  `range.start`, `diagnostics` by `(file, range.start, code)`.
- `generationId` is **project-monotonic**, not daemon-monotonic: it is
  allocated under the publication lock via the max-on-disk rule (see
  Transactionality), so concurrent writers can never mint the same id. `changed: false` responses repeat the
  daemon's own last outcome; a daemon does not watch disk, so
  supersession by another process is only observed at its next real
  publication — bindings needing cross-process coherence compare the
  manifest's `generationId`.
- `callsites` lists, per embedding host file, each **expression** the
  extractor found — the range spans the entire callsite (the whole
  `` dsql(`…`) ``, not the content between the backticks) — and the
  definitions its embedded document declares. Expression ranges within
  a file never overlap; definitions share their expression's range by
  construction. Rewrite rules live under Callsites and freshness.
  Embedded definitions' *artifact metadata* additionally records
  `content_range` on their source-map entry — the exact byte range of
  the document content between the backticks — so renderers that key
  generated types by source text slice it from the host by extractor
  authority instead of re-detecting anything (absent for plain `.dsql`
  files; additive to manifest version 2).
- `diagnostics` is a **complete snapshot on every compile response**
  (success and `Diagnostics` failure): warnings and infos appear here on
  success. Bindings replace their entire displayed diagnostic state with
  each snapshot — anything absent has been resolved. Non-compile
  responses (`initialize`, `shutdown`, protocol errors) carry no
  snapshot.
- The metadata types are the ones published by `dsql metadata-schema` /
  `dsql metadata-typescript`; this payload embeds those shapes rather
  than inventing parallel ones.

### Ranges and hashes

- All ranges in this protocol are **zero-based, half-open byte offsets
  into the file's exact UTF-8 bytes**, guaranteed to fall on code-point
  boundaries. Expression ranges within one file never overlap.
  Consumers holding UTF-16 strings (JavaScript) must convert offsets
  before slicing, and MUST splice in descending range order.
- `contentHash` is SHA-256 over the file's exact bytes as read by the
  extractor — no BOM stripping, no newline normalization; a BOM or CRLF
  difference is a real difference. Rendered as
  `{ "algorithm": "sha256", "value": "<lowercase hex>" }`. The
  algorithm is fixed by `protocolVersion` (changing it is a protocol
  bump), so it is not separately negotiated or recorded in the manifest.

## Callsites and freshness

Consumers MUST rewrite callsites exclusively from `callsites` ranges —
no scanning, no regexes in the consumer.

- **Ranges apply only to the raw, unmodified source file** — the exact
  bytes hashed into `contentHash`. A binding must run before any
  source-altering transform in its pipeline (in Vite terms:
  `enforce: "pre"`), or decline to transform.
- Before splicing, the consumer hashes the buffer it is about to
  transform. On match, it splices. On mismatch, it sends
  `filesChanged [path]` and retries against the fresh response
  **exactly once**; if the buffer still mismatches, the buffer is not
  the saved file (an upstream transform or unsaved state) and the
  binding MUST fail that file's transform with a deterministic
  stale-buffer error rather than retry indefinitely.
- A well-behaved watcher loop (change → `filesChanged` → transform)
  makes mismatches rare; the hash check is the safety net, not the
  mechanism.

### Rewrite contract

The daemon supplies *where* (expression ranges) and *what exists*
(definition ids); the **renderer** supplies *what to write there*. The
renderer is consumer-side host-language code generation (the POC's TS
renderers being the model): the binding invokes it after every changed
successful compile with the full compile result, and it returns a
render map:

```jsonc
{
  "modules": [
    {
      "id": "frontend/operation/TitlePanel",
      "module": "src/generated/dsql/frontend/queries/index.ts",
      "export": "TitlePanel"                           // named export
    }
  ],
  "ownedRoots": ["src/generated/dsql"],  // directories the renderer owns
  "files": ["src/generated/dsql/frontend/queries/index.ts", "…"]
}
```

- `module` is a **project-base-relative file path**, not an import
  specifier: the renderer states which file exports the operation, and
  the binding derives the specifier its host understands (root-relative,
  `/@fs/` absolute, require path, …) — specifier semantics are host
  knowledge a renderer must not hardcode.
- The binding rewrites an expression by replacing its range with a
  reference to the mapped export and ensuring the corresponding import
  exists in the transformed module. How the import is materialized
  (hoisted import statement, require, etc.) is binding-specific; the
  *mapping* `id → (module, export)` is the renderer's contract.
- **Definition rules per expression** (v1):
  - exactly one `query` → rewrite to that query's export reference
    (accompanying fragments in the same document are fine — they need
    no expression-level rewrite of their own);
  - more than one `query` → daemon-side `Diagnostics` error (ambiguous
    rewrite target);
  - fragments only → daemon-side `Diagnostics` error. Leaving the raw
    `dsql(…)` expression in shipped code is not sound (it evaluates
    against a runtime that may not exist), and the fragment-handle
    runtime contract is undecided — rejected until the T2 runtime
    design settles it. Fragment-only *documents* remain fully supported
    in plain `.dsql` files.
- `ownedRoots` is **renderer configuration, known before any
  invocation** (the render result repeats it as a consistency check,
  not as the source of truth) — otherwise the initial full compile
  could discover generated code as input before the first render ever
  reported its roots. A binding whose renderer owns any roots MUST pass
  them as `initialize`'s `excludeRoots` so the daemon reserves them on
  its side (see Host generator command); omitting `excludeRoots` is
  correct only for renderers that write no files. Those roots plus
  `buildDir` plus `initialize`'s `generatorOutputs` form the binding's
  watch exclusions, so generation never retriggers itself. `files` is
  informational (precise invalidation); exclusion must not depend on
  it, because a renderer's *next* run can write files the previous run
  did not list.
- After a recompile+render, the binding triggers its host's reload
  semantics; module-graph invalidation is a binding-quality concern,
  full reload is the acceptable default.
- A binding MUST NOT transform any file until the initial compile and
  render have both succeeded, and MUST swap its cached compile result
  and render map atomically — only on success, never partially.

## Transactionality

Artifact files are **content-addressed**: an artifact writes to
`operations/<name>.<hash>.json` (likewise fragments), where `<hash>` is
the first 16 lowercase hex characters of the SHA-256 of the serialized
artifact — the same hash recorded in full in its manifest entry (the
manifest's per-artifact hash field moves to SHA-256 as part of this
format bump; the current 64-bit FNV-1a is not collision-safe enough to
address files by). If the target path already exists, its bytes are
compared: identical → the write is skipped; different → a hard
collision error (`Io`), never a silent overwrite. Distinct generations
therefore never overwrite each other's files.

Manifests are two files:

- `manifest.<generationId>.json` — an **immutable per-generation
  manifest**; never rewritten once committed.
- `manifest.json` — the fixed-path **current-generation pointer**, whose
  content is the same document; committing means atomically renaming a
  flushed temporary file over it. Every file write in publication
  (artifacts included) goes through temp-file + rename so an interrupted
  write can never strand a partial file at its final address.

Together with the manifest's new `generationId` field this is an
explicit **manifest format version bump**; manifest consumers follow
paths from manifest entries and never glob the build directory.

Publication order for every changed successful compile:

1. Settle and assemble an immutable generation snapshot in memory.
2. Validate: language diagnostics (errors → `Diagnostics`, nothing
   written), then artifact-path collisions (`ArtifactCollision`,
   nothing written).
3. Acquire the **publication lock** — an exclusive advisory lock file
   under `buildDir`, shared by every writer (`dsql daemon` instances
   and `dsql generate` alike). Bounded wait; on timeout the request
   fails with `PublicationLocked` and nothing is written. Concurrent
   *processes* are allowed; concurrent *publication* is not.
4. Allocate this generation's id under the lock as
   `max(committed manifest.json generationId, every manifest.<id>.json
   present) + 1`. A crash between writing an immutable manifest and
   committing the pointer strands a manifest file; scanning existing ids
   makes the stranded id skipped, never reused — skipped ids are
   harmless, reused ids are not. Project-monotonic regardless of how
   many writers exist.
5. Write the generation's content-addressed artifact files, then
   `manifest.<generationId>.json`, then commit by renaming over
   `manifest.json`.
6. Release the lock; run the configured host generator command (if
   any), with `DSQL_MANIFEST` pointing at the **immutable
   per-generation manifest** — so a concurrent publication replacing
   `manifest.json` mid-generator is invisible to it.
7. Respond.
8. Maintenance, best-effort and strictly post-commit: the publisher
   records which generation `manifest.json` referenced immediately
   before its own commit (its predecessor), **re-acquires the
   publication lock**, and — only if `manifest.json` still points at
   its own generation — prunes artifact files and per-generation
   manifests referenced by neither its generation nor that recorded
   predecessor. If the pointer was superseded meanwhile, it skips
   pruning entirely and leaves maintenance to the newer publisher —
   this is what keeps the *actual* predecessor chain intact instead of
   an arbitrary one. The lock is what keeps pruning from eating another
   publisher's freshly written, not-yet-committed files. A pruning
   failure (or lock timeout) logs a warning to stderr and never fails
   the compile — the commit already happened.

Consequences, stated explicitly:

- An `Io` failure during step 5 leaves the previous `manifest.json` —
  and every file it references — fully intact: the previous generation
  remains current by construction (content-addressing + atomic rename),
  not by luck.
- A host generator failure (step 6) happens **after** the build tree
  committed: the response is `GeneratorFailed` with the new
  `generationId` and its per-generation manifest path in `data`. The
  build tree on disk is the *new* generation; only the generator's own
  outputs are suspect. Consumers treat any `error` as "no usable new
  compile result for rendering" regardless.
- Current-plus-previous retention means a generator survives one
  concurrent publication while it runs. A generator still running after
  *two* subsequent publications may observe pruned files; that
  manifests as `GeneratorFailed`, which is the honest outcome for a
  generator that slow inside a watch loop.

## Consumer responsibilities

A binding (the Vite plugin being the first) owns:

- **Lifecycle**: spawn `dsql daemon` lazily on first need, `initialize`,
  `shutdown` (with timeout, then kill) on teardown; restart with backoff
  and full state invalidation on unexpected exit.
- **Watching**: watch the whole `projectBase` recursively and forward
  every create/modify/delete/rename (as delete+create) via
  `filesChanged`, excluding only the renderer's `ownedRoots`,
  `buildDir`, and `generatorOutputs`. Scope relevance is the daemon's
  judgment, not the watcher's — `sourceFileScopes` is informational
  (e.g. for UX), not a watch list, because it cannot name files that
  don't exist yet.
- **In-flight dedup**: at most one outstanding compile; coalesce every
  change that arrives meanwhile into the next request's `paths`.
- **Rewriting**: per the rewrite contract and freshness rules above.
- **Diagnostics**: present each compile response's complete snapshot;
  clear everything not in the latest snapshot.

Everything else — detection, scope resolution, analysis, SQL, artifact
assembly — is the daemon's.

## Host generator command

The configured command (`[generate.typescript] cmd` + `DSQL_MANIFEST`
pointing at the immutable per-generation manifest) is still honored on
every daemon-driven compile, with one addition: a project that enables
a generator command **and** is driven by a daemon consumer must declare
the command's output directories in configuration
(e.g. `[generate.typescript] outputs = ["src/generated"]`). `initialize`
returns them as `generatorOutputs` for watch exclusion; a daemon compile
**skips** an enabled generator command that declares no outputs and
says so with a warning in the `diagnostics` snapshot — an unexcludable
generator inside a watch loop is an infinite regeneration cycle, which
is worse than a skipped generator. One-shot `dsql generate` (no watcher
in the loop) runs it regardless.

Declared outputs are validated at project load: each must be a
normalized project-base-relative directory, must not be the project
base itself, must be **disjoint from `configPath`, `schemaDir`, and
`buildDir` in both directions** (neither ancestor nor descendant —
`dsql/schema/generated` would hide schema changes just as surely as
`dsql/`), and must not equal or contain the static (non-glob) prefix
of any configured scope's document pattern (one direction only: a
reserved directory *inside* a broad scope, like `src/generated` under
`src/**/*.ts`, is exactly the point) — an output declaration that
would swallow the project's own inputs is a configuration error, since
it would silently stop the binding from watching real sources. Paths
outside the project base are rejected the same way.

Generator outputs, the consumer's `excludeRoots`, and `buildDir` are
**reserved roots**: normatively excluded not just from consumer
watching but from the daemon's own document discovery and
`filesChanged` subtree reconciliation. Generated code is never project
input, no matter which side generated it.

Command generators see only the flat on-disk manifest: **scope-aware
(grouped) generation is daemon-channel-only in v1.** If the on-disk
manifest later grows group references (the scope-qualified artifact
layout follow-up in docs/issues.md), that is a further manifest version
bump, independent of this protocol.

## Relationship to other surfaces

- **LSP**: same engine, different protocol and lifecycle. The LSP serves
  editors (positions, hovers, streams of keystrokes); the daemon serves
  build tools (compiles, artifacts, file-granular changes). They do not
  share a process in version 1.
- **CLI**: `dsql generate` remains the one-shot equivalent of `compile`,
  and participates in the same publication lock; CI needs no daemon.

## Open questions

- Inline buffer contents in `filesChanged` (unsaved editor state) for
  consumers that transform pre-save? Version 1 says no — build tools
  operate on saved files; the stale-buffer error makes the boundary
  visible instead of silent.
- The fragment-handle runtime contract (would relax the fragment-only
  expression rejection) — T2 design session.
- Cancellation: a long compile cannot be aborted by a newer
  `filesChanged` in v1 (strict FIFO). If compile latency ever warrants
  it, cancellation needs an explicit protocol addition, not implicit
  supersession.
