# Agent Guidelines

## Workflow

- Read [`docs/architecture/compiler.md`](docs/architecture/compiler.md) before changing compiler, frontend analysis, diagnostics, formatting, generation, LSP, or source/project ownership.
- Read relevant docs in [`docs/proposals`](docs/proposals) and [`docs/spec`](docs/spec) before implementing language features.
- ALWAYS check the working tree before staging or committing.
- ALWAYS preserve user changes. Do not revert, overwrite, or reformat unrelated files unless explicitly asked.
- ALWAYS keep commits focused around one coherent behavior or infrastructure change.
- NEVER commit, amend, create a checkpoint commit, or otherwise modify git history unless the user explicitly asks for a commit in the current conversation.
- NEVER treat approval to implement, continue, checkpoint, verify, or resolve review comments as approval to commit.
- If the user asks for a commit, first check the working tree, stage only the intended files, and then commit.
- Use conventional commit messages, such as `feat: ...`, `fix: ...`, `test: ...`, `docs: ...`, `refactor: ...`, or `chore: ...`.
- Check the working tree before the final response.

## Tools And Verification

- PREFER running specific tests over running the entire test suite.
- ALWAYS run formatting, focused tests, and relevant checks before handing off code changes when practical.
- PREFER `cargo test -p <package> <test-name>` or the nearest package-level test over `cargo test --workspace` while iterating.
- Use `cargo test --workspace` when a change crosses compiler, frontend, generate, LSP, or CLI boundaries and a full pass is practical.
- PREFER `cargo check --workspace` for broad compile verification after API or type changes.
- Run clippy with warnings denied when practical; NEVER assume clippy warnings are pre-existing.
- NEVER perform builds with the release profile unless asked or reproducing performance issues.
- When making Windows-specific changes from Unix, use `cargo xwin clippy` to check compilation when available; if it cannot be run, say so.
- NEVER update all dependencies in the lockfile. ALWAYS use `cargo update --precise` for lockfile changes.

## Secondary Goal Review

- For large ongoing refactors, create or use a task-prefixed goal file in `local-docs`, such as `local-docs/<task>-goal.md`.
- Before handing off a substantial slice or preparing a commit, run a secondary review using the goal file that matches the active task and a task-specific prompt, for example:
  `cat local-docs/<task>-goal.md | codex exec "Using the provided context, solve this problem: review the current diff against this goal and report concrete deviations, missing coverage, and risky shortcuts."`
- Replace both `<task>-goal.md` and the quoted prompt with the current task. Do not hardcode one goal file for unrelated work.
- Treat the secondary review output as review input: address valid findings or mention unresolved risks in the handoff.

## Testing

- For issues and bug reports, add or run a focused test that reproduces the behavior before starting the fix.
- For compiler, language, frontend analysis, diagnostics, formatting, generation, LSP, source/project ownership, and other user-visible dsql behavior, ALWAYS use integration tests. Use `insta` snapshots and copy nearby fixture/query patterns where possible.
- Smaller targeted unit tests are acceptable for isolated utilities, data structures, parsers, or algorithms with non-trivial internal logic, when the behavior is not better covered through a compiler-facing integration path.
- NEVER test external crate functionality or simple encode/decode behavior with `string contains` assertions. Tests should protect dsql semantics, integration behavior, or project-specific boundaries.

## Rust Style

- PREFER top-level imports over local imports or fully qualified names.
- PREFER clear variable names over abbreviations, e.g. `version` instead of `ver`.
- PREFER patterns like `if let` to handle fallibility.
- PREFER let chains (`if let` combined with `&&`) over nested `if let` statements.
- AVOID `panic!`, `unreachable!`, `.unwrap()`, unsafe code, and clippy rule ignores.
- PREFER `#[expect(...)]` over `#[allow(...)]` if a lint must be disabled.
- ALWAYS write `SAFETY` comments following the surrounding style when writing unsafe code.
- PREFER [`TypeName`] references when writing Rust doc comments.
- Add succinct comments only where the code is not self-explanatory.

## Project Patterns

- Prefer existing project patterns and local helper APIs over new abstractions.
- Add abstractions only when they remove real duplication or make a provider/runtime boundary clearer.
- Bind repeated or semantically important strings to a single source of truth, or colocate them with the enum/type/API they describe.
- Do not randomly hardcode keywords, metadata labels, path segments, protocol fields, or artifact names at call sites.
- Derive or implement reflection/serialization traits on shared data types where the existing code expects them.
- Before version 1.0, refactors are replacement changes by default. If a refactor touches an API, format, data model, or behavior, update all affected callers to the new shape and remove the old path. Do not add compatibility bridges, migration layers, aliases, or old-to-new adapters unless the user explicitly asks for backward compatibility.

## Documentation

- Prefer literate, self-explaining compiler code for new architecture-facing APIs.
- ALWAYS add proper doc comments to every new or changed compiler-facing `pub` or `pub(crate)` item, tracked Picante query, shared compiler data type, and non-obvious private function that encodes compiler architecture or stage boundaries.
- Treat `pub(crate)` architecture APIs as documentation-required even when they live in private modules or are not re-exported outside the crate.
- Doc comments should explain what the item represents, why it exists, and which layer owns it when that context is not obvious. Do not add redundant comments for simple/self-evident functions, and avoid comments that merely repeat the function name or restate obvious parameter types.

## Issues

- Issues are tracked as markdown files in `issues/`.
- Create new issues with `scripts/create-issue.sh <issue title>`.
- Keep issue files concise and standalone enough to understand without Peers threads, chat logs, or other external review state.
- Do not reference Peers thread IDs from issue files.
- Set `Status` to `Done` when resolved.
- Find open issues with `rg 'Status:.*Open' issues/`.
