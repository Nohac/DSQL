# Decouple TypeScript rewriting from dsql internals

**ID:** e795c48b | **Status:** Open | **Created:** 2026-07-18T00:13:24+02:00

The TypeScript/Vite binding currently knows more about source ownership and
dsql definition semantics than a build adapter should:

- `vite.ts` uses a hard-coded `[cm]?[jt]sx?` extension gate before consulting
  daemon callsites. This duplicates the project extraction configuration and
  means a new extractor or host extension also requires a Vite change.
- `rewrite.ts` maps extensions to `ts.ScriptKind` itself. The parser is needed
  to insert static imports after a shebang and directive prologue, but the
  hand-written mapping duplicates TypeScript's own filename inference. The
  current call also passes the synthetic filename `module.ts`, forcing the
  adapter to reconstruct information it already received as `path`.
- `rewrite.ts` filters callsite definitions by `kind === "query"` and checks
  that exactly one exists. This both reimplements language rules and encodes a
  parity regression: the POC rewrote a named fragment-only expression to its
  generated fragment handle, while the current daemon rejects it. The binding
  should consume one opaque operation-or-fragment rewrite target.

Make the daemon/extractor boundary the single source of truth for which host
files contain rewritable regions. A compile result should provide each
expression's byte range, content hash, and direct artifact/rewrite-target id.
That target may be a generated operation or fragment handle; fragment-only
embedded expressions must not be rejected merely because they have no query.
If rewriting requires a host syntax or extractor identity that cannot be
inferred from the path, publish it with the callsite instead of rediscovering it
from an extension in Vite.

The Vite transform should normalize the module id and look up a daemon-owned
callsite without a source-extension allowlist. A file with no callsite passes
through untouched. The TypeScript rewriter should run only for a callsite owned
by the TypeScript embedder and should either pass the real filename to
`ts.createSourceFile` and use TypeScript's script-kind inference, or consume an
explicit daemon-provided dialect.

Keep genuinely host-side responsibilities in the binding:

- verifying that the Vite buffer matches the daemon content hash;
- applying daemon byte ranges in descending order;
- mapping opaque artifact ids through the renderer's module/export map;
- deriving Vite import specifiers;
- selecting collision-free local bindings and placing valid static imports.

Those operations depend on the current host buffer, renderer, TypeScript
syntax, or Vite module semantics. Moving them into the language compiler would
couple the daemon in the opposite direction.

Update the build-daemon rewrite contract and TypeScript protocol types so this
ownership is explicit. Audit `vite.ts`, `rewrite.ts`, and their public inputs
for other language facts that can be replaced by the adapter-facing rewrite
contract.

Acceptance coverage should include:

- a daemon-owned TypeScript host outside the current extension regex rewrites
  without changing the Vite plugin;
- a normal TypeScript/JavaScript module without callsites passes through;
- TSX/JSX and shebang/directive prologues still receive imports in the correct
  location, with TypeScript rather than dsql owning dialect parsing;
- the binding does not inspect query/fragment kinds or count definitions;
- a named fragment-only expression rewrites to its generated typed fragment
  handle, restoring the POC behavior and removing `EmbeddedExpressionShape`'s
  query-only restriction;
- stale-buffer hashing, multibyte byte ranges, renderer mappings, import-name
  collisions, and Vite specifier behavior retain their existing coverage.
