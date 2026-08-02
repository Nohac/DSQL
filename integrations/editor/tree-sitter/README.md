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

The portable tests check the literal surface, field-bearing corpus trees, semantic
capture placement, capture-name validity, and every repository `.dsql` fixture.
`test:corpus` owns the placement assertions; `check:captures` validates the highlight
capture-name allowlist. Generated
`src/parser.c`, `src/grammar.json`, and `src/node-types.json` are committed; local parser
libraries and dependency directories are ignored.

## Neovim

[`queries/indents.scm`](queries/indents.scm) provides structural indentation through
the current `nvim-treesitter` indentation engine. Run its editor-specific test with
Neovim and `nvim-treesitter` installed:

```sh
bun run test:indents
```

Set `DSQL_NVIM_TREESITTER_PATH` to a local `nvim-treesitter` checkout when it is not
available through Neovim's package path.

For a local parser installation, register this directory with `nvim-treesitter` and
enable its indentation expression for DSQL buffers:

```lua
local parser_dir = "/absolute/path/to/dsql/integrations/editor/tree-sitter"

require("nvim-treesitter.parsers").dsql = {
  install_info = {
    path = parser_dir,
    queries = "queries",
    generate = false,
    -- path overrides these fields; empty values satisfy InstallInfo type checkers.
    url = "",
    revision = "",
  },
  tier = 2,
}

require("nvim-treesitter").install({ "dsql" }):wait(30000)
vim.filetype.add({ extension = { dsql = "dsql" } })

vim.api.nvim_create_autocmd("FileType", {
  pattern = "dsql",
  callback = function()
    vim.treesitter.start()
    vim.bo.indentexpr = "v:lua.require'nvim-treesitter'.indentexpr()"
  end,
})
```

The installer links this package's `queries` directory as Neovim's
`queries/dsql/`; `captures.txt` is a Tree-sitter CLI allowlist and is ignored by
Neovim. Without the installer, copy or link `highlights.scm` and `indents.scm` into
`queries/dsql/` on any directory in `runtimepath`.
