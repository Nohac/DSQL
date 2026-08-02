# Contributing

## Layout

- `docs/plan.md` — the architecture plan; read it before structural changes.
- `crates/` — the workspace crates; `dsql-core` is the language itself.
- `vendor/` — vendored dependencies (`lelwel`, `logos`), added as git subtrees
  (see docs/plan.md for the rationale). `vendor/PATCHES.md` is the ledger of
  local changes.

## Building and checking

- `cargo check --workspace` after API changes; `cargo check -p <crate>` while
  iterating.
- `cargo clippy --workspace --all-targets` must be warning-free; new warnings
  are never pre-existing.
- Keep the storage-independent compiler surface WASM-compatible. Check it with
  `cargo check -p dsql-generate --no-default-features --target wasm32-unknown-unknown --lib`;
  native project loading and publication belong behind the crate's `native`
  module boundary.
- Never build with `--release` unless reproducing a performance issue.
- The parser is generated at build time from `crates/dsql-core/src/grammar/dsql.llw`
  by the vendored lelwel; editing the `.llw` regenerates it on the next build.
  Adding a grammar rule intentionally breaks compilation until an entity claims
  it in the `lower_rule` dispatch.
- The editor grammar under `integrations/editor/tree-sitter` is maintained manually.
  Changes to `dsql.llw` or the lexer patterns in `crates/dsql-core/build.rs` must update
  it and pass `bun run check:surface`, `bun run check:captures`, and
  `bun run test:corpus` from that directory. The surface check would, for example,
  have caught the removed double sigil and its percent replacement.

## Testing

- Integration tests live in `tests/it/` inside each crate, one harness binary
  with a module per area. Run them with
  `cargo test -p <crate> --test it <module or test filter>`.
- Prefer `insta` snapshots over assertions for anything user-visible (CST,
  facts, diagnostics, plans, SQL, formatter output, service responses).
  Review with `cargo insta review`; never hand-edit `.snap` files.
- Snapshot settled bowl state only, with stable ordering — never mid-settle
  state.
- Fixture queries live under `tests/it/queries/{valid,invalid}/`; copy the
  style of nearby fixtures when adding cases.
- Never test external crate/library functionality — only our own behavior
  (vendored patches we maintain count as ours). Trivial tests of the
  "serialize a struct, check a string exists" kind are not useful; test
  observable pipeline behavior instead.

## Style

- Every `pub` and `pub(crate)` item gets a doc comment: what it represents and
  which layer owns it. Use [`TypeName`] link references.
- Language entities are vertical slices: one file per concept under
  `entities/`, co-locating facts, lowering, checks, and service contributions.
  No stage orchestration or project-level logic inside an entity file.
- Bind repeated strings (keywords, directive names, diagnostic codes) to a
  single source of truth; never scatter literals across call sites.
- Name `Bowl` variables and parameters `bowl`, not `db` (`bowl.scoop` reads
  the way the engine intends).
- Keep entrypoint files (`lib.rs`, `mod.rs`, `main.rs`) slim: module
  declarations, re-exports, and pure composition only — function bodies live
  in named modules.
- Prefer narrow components. When a value describes the entity as a whole and
  a cross-cutting component already exists for it (`Span`, `BelongsToFile`,
  `Severity`), attach that component to the entity instead of embedding the
  field in a fact struct.
- Pre-1.0: refactors are replacement changes — no compatibility bridges.

## Dependencies

- `bowl` (porridge) is a git dependency pinned to a rev; bump deliberately and
  in a dedicated commit.
- Lockfile changes only via `cargo update --precise <pkg>@<version>`.

## Vendored crates

- Every local change to a crate under `vendor/` MUST be recorded in
  `vendor/PATCHES.md` (what, where, why) in the same commit, with the commit
  subject prefixed `vendor(<crate>):`. If it is not in the ledger, it does
  not exist.
- Keep vendor changes additive (new hooks, new emit passes) rather than
  rewriting upstream code, so `git subtree pull` merges stay tractable and
  patches remain upstreamable.
- Update a subtree with
  `git subtree pull --prefix vendor/<crate> <upstream-url> <tag> --squash`,
  in a dedicated commit, and record the new upstream rev in the ledger.
