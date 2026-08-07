# Editor Grammars

The DSQL Tree-sitter grammar is maintained manually under [`tree-sitter/`](tree-sitter/).
It is an editor parser: it favors stable recovery and semantic node roles while the Rust
parser remains the authority for language validity.

The previous Prismers output was removed because its grammar did not expose enough
structure for semantic highlighting. Do not regenerate a Tree-sitter grammar from the
Lelwel source. The line-comment spelling (`#`), bracket pairs (`{}`, `()`, `[]`), and
auto-closing pairs (`{}`, `()`, `[]`, `""`) are recorded here for future editor bindings.
