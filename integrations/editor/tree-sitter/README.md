# Tree-sitter DSQL

This hand-maintained grammar mirrors the syntax in
[`crates/dsql-core/src/grammar/dsql.llw`](../../../crates/dsql-core/src/grammar/dsql.llw)
for editor parsing and highlighting. It may accept incomplete or otherwise invalid DSQL
to preserve useful trees during editing; the Rust parser owns validation.

`language-surface.mjs` and `scripts/check-language-surface.mjs` keep literal tokens,
terminal names, and terminal regex spellings synchronized with the compiler grammar and
`crates/dsql-core/build.rs`.

Run the complete grammar gate from this directory:

```sh
tree-sitter generate
bun run test
```

The tests check the literal surface, field-bearing corpus trees, semantic capture
placement, capture-name validity, and every repository `.dsql` fixture. `test:corpus`
owns the placement assertions; `check:captures` validates the capture-name allowlist.
Generated
`src/parser.c`, `src/grammar.json`, and `src/node-types.json` are committed; local parser
libraries and dependency directories are ignored.
