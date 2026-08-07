# Editor Grammars

The DSQL Tree-sitter grammar is maintained manually under [`tree-sitter/`](tree-sitter/).
It is an editor parser: it favors stable recovery and semantic node roles while the Rust
parser remains the authority for language validity.

The previous Prismers output was removed because its grammar did not expose enough
structure for semantic highlighting. Do not regenerate a Tree-sitter grammar from the
Lelwel source. The line-comment spelling (`#`), bracket pairs (`{}`, `()`, `[]`), and
auto-closing pairs (`{}`, `()`, `[]`, `""`) are recorded here for future editor bindings.

## Neovim diagnostics

DSQL edits can change diagnostics in another open buffer. Neovim defers diagnostics while
insert mode is active when `update_in_insert = false`, but its deferred `InsertLeave` refresh is
buffer-local. A diagnostic published for buffer B while inserting in buffer A can therefore stay
cached but hidden until B receives another publication.

Configure the DSQL client's diagnostic namespace to update during insert mode while leaving the
global diagnostic policy unchanged:

```lua
vim.lsp.config('dsql', {
  -- cmd, filetypes, root_dir, ...
  on_attach = function(client)
    local namespace = vim.lsp.diagnostic.get_namespace(client.id)
    vim.diagnostic.config({ update_in_insert = true }, namespace)
  end,
})
```
