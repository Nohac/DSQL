# Contributing

## Layout

- `docs/plan.md` — the architecture plan; read it before structural changes.
- `crates/` — the workspace crates; `dsql-core` is the language itself.
- `vendor/lelwel/` — vendored, patched parser generator (see docs/plan.md for
  why it is vendored and what the patches are).

## Building and checking

- `cargo check --workspace` after API changes; `cargo check -p <crate>` while
  iterating.
- `cargo clippy --workspace --all-targets` must be warning-free; new warnings
  are never pre-existing.
- Never build with `--release` unless reproducing a performance issue.
- The parser is generated at build time from `crates/dsql-core/src/grammar/dsql.llw`
  by the vendored lelwel; editing the `.llw` regenerates it on the next build.
  Adding a grammar rule intentionally breaks compilation until an entity claims
  it in the `lower_rule` dispatch.

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

## Style

- Every `pub` and `pub(crate)` item gets a doc comment: what it represents and
  which layer owns it. Use [`TypeName`] link references.
- Language entities are vertical slices: one file per concept under
  `entities/`, co-locating facts, lowering, checks, and service contributions.
  No stage orchestration or project-level logic inside an entity file.
- Bind repeated strings (keywords, directive names, diagnostic codes) to a
  single source of truth; never scatter literals across call sites.
- Pre-1.0: refactors are replacement changes — no compatibility bridges.

## Dependencies

- `bowl` (porridge) is a git dependency pinned to a rev; bump deliberately and
  in a dedicated commit.
- Lockfile changes only via `cargo update --precise <pkg>@<version>`.
