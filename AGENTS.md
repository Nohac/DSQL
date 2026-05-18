# Agent Guidelines

## Engineering Direction

- Keep syntax and semantics separate. Parsing should describe source structure; lowering should extract names and structure; checking should resolve against catalog and schema information.
- Treat the CST and original source text as the source of truth for rewriting and formatting. The AST is for typed access and analysis, not for reconstructing user text.
- Formatter changes must be conservative. Preserve comments, trivia, malformed regions, and unknown syntax unless the formatter has enough structure to rewrite safely.
- Store spans as byte ranges internally. Convert to editor protocol positions only at the protocol boundary.
- Use interned names and IDs for identity, comparison, and semantic indexing, not for printing source text.
- Return diagnostics explicitly from analysis stages and aggregate them in source order where possible.
- Keep parser-generated types behind local wrappers or facades. Do not let generated parser APIs leak into unrelated layers.
- Keep the compiler core pure and reusable. Stateful incremental analysis belongs at the frontend boundary.
- CLI and LSP code should be thin adapters over the analysis API. They should not contain language rules.
- The LSP/editor layer should own editable Rope state, apply text edits through Rope range operations, and publish new immutable revisions to analysis.
- Immutable snapshots may use `Arc<str>` for now. Avoid copying source text when a direct Rope-backed parser path becomes practical.
- Do not use `Arc<Mutex<_>>` or `Arc<RwLock<_>>` as the normal application architecture. If mutable concurrent maps are needed, prefer `DashMap`.
- Analysis host handles should be cheaply clonable and use internal mutability through the intended query/runtime boundaries.
- Catalog access should go through provider-style interfaces so swapping a hardcoded catalog for a PostgreSQL-backed catalog is a provider change.
- Schema-qualified references should be supported. Unqualified table references default to `public`.
- Relation names are table names with foreign-key relationships. Do not singularize, pluralize, or otherwise rewrite relation names.
- Hover, completion, checking, diagnostics, formatting, and CLI behavior should all use the same analysis surface instead of duplicating logic.

## Parser And Grammar

- Prefer extending the grammar over adding ad hoc text parsing.
- Keep the grammar small and explicit until the language spec settles.
- Query bodies should look SQL-like where applicable, for example `where ...` rather than `where: ...`.
- Qualified names should be parsed structurally and preserved in spans/text.
- If parser support for Rope chunks becomes available, pursue a path that avoids full source allocation across all layers.

## Formatting

- The formatter must operate from the CST/source spans, not from the AST.
- If a file has parse errors, refuse formatting or preserve malformed subtrees rather than producing guessed output.
- Do not normalize trivia or comments unless the behavior is deliberate and covered by tests.

## Diagnostics And LSP

- Diagnostics should include precise byte ranges and a stable source category.
- Use Rope APIs for byte/line/character conversion in editor-facing code.
- LSP document changes should mutate the stored Rope, then publish a new revision for analysis.
- Completion and hover should be contextual and catalog-aware, including inside fragments.
- Avoid keeping a second compiler-state mirror in the LSP. Use the shared analysis host as the source of truth.

## Dependencies And Abstractions

- Prefer existing project patterns and local helper APIs over new abstractions.
- Add abstractions only when they remove real duplication or make a provider/runtime boundary clearer.
- Keep Picante details behind the frontend analysis API. Do not expose query ingredients or runtime internals to adapters.
- Derive or implement reflection/serialization traits on shared data types where the existing code expects them.
- Before version 1.0, do not preserve backward compatibility by default. Prefer clean formats, APIs, and data models over migration code unless the user explicitly asks for compatibility.

## Testing And Verification

- For bug reports, first add or run a focused failing regression test that
  demonstrates the reported behavior. Confirm the failure before changing
  implementation, unless the user explicitly asks for exploratory or speculative
  edits.
- Add focused regression tests for language behavior, diagnostics, and editor-facing analysis when changing those areas.
- For TypeScript inference bugs, inspect the actual checker type rather than relying only on emitted errors. A small script using the `typescript` package can load `tsconfig.json`, create a `Program`, call `checker.getTypeAtLocation(...)`, and print `checker.typeToString(...)` for the node that would be hovered in an editor. This is often simpler than manually driving the LSP protocol and gives the same actionable type information.
- When debugging generated TypeScript, regenerate the consuming fixture/app and inspect the generated files before changing the generator. If a value unexpectedly becomes `never`, add a type-level regression assertion such as `type IsNever<T> = [T] extends [never] ? true : false` and `type AssertFalse<T extends false> = T`; a plain `satisfies` assertion can miss this because `never` satisfies every type.
- Do not test external crate functionality. Tests should protect dsql semantics, integration behavior, or project-specific boundaries, not verify that dependencies parse defaults or expose documented behavior.
- Avoid tests that only restate library behavior without protecting project semantics.
- Before committing code changes, run formatting, tests, and clippy with warnings denied.
- If a verification command cannot be run, say why in the final response.

## Git And Commits

- Use conventional commit messages, such as `feat: ...`, `fix: ...`, `test: ...`, `docs: ...`, `refactor: ...`, or `chore: ...`.
- Keep commits focused around one coherent behavior or infrastructure change.
- Do not mix unrelated cleanup with feature or bug-fix commits.
- Do not revert or overwrite user changes unless explicitly asked.
- Check the working tree before staging and before the final response.
