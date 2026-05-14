# TODO

## Definition Graph And Multi-File Passes

- Make every output-affecting pass consume the same definition graph and resolver model.
  - Checking already resolves fragments across files through frontend definition inputs.
  - Planning is still file-local and currently builds a `FragmentMap` only from the current `SourceFile`.
  - Output generation should not grow a separate resolver path.
  - Target shape: check, lint, plan, and generation all operate on definition records plus a resolver supplied by the frontend/host layer.

- Add definition-level planning inputs in the frontend, matching the current definition-level check/lint inputs.
  - Query planning should receive the query record and resolved fragment inputs.
  - Fragment spreads in file B must plan correctly when the fragment is defined in file A.
  - Avoid re-planning unrelated queries when a dependency hash/input is unchanged.

## Fragment Expansion Semantics

- Centralize fragment spread expansion with cycle protection.
  - Checking currently validates spread existence and `on` target compatibility, but does not fully recurse through the spread selections in the use context.
  - Planning currently expands fragments recursively, but without a visiting set.
  - A shared expansion/check helper should track the fragment stack and return deterministic cycle diagnostics instead of risking recursion bugs.

- Check duplicate output keys after fragment expansion.
  - Duplicate detection currently skips fragment spread selections.
  - A query can become invalid only after expanding a spread, for example if a local field and a fragment field produce the same response key.
  - Aliases and qualified relation names should still use the existing response-key rules.

- Ensure linting handles spread-expanded selections consistently.
  - Lint currently owns diagnostics at the fragment definition site, which is good for many cases.
  - Use-context lints may still be needed when the lint depends on the actual parent relation/table context.
  - Keep source ranges stable: fragment-owned problems should point at the fragment; use-site problems should point at the spread.

## Fragment Identity And Namespacing

- Decide and enforce the fragment namespace model.
  - Current frontend and core maps key fragments by name only.
  - Two open files defining the same fragment name are currently order-dependent.
  - If fragments are global, add deterministic duplicate diagnostics across files.
  - If fragments are file/module scoped, include the scope in the fragment key and resolver lookup.

- Avoid silent overwrites in `FragmentMap`.
  - Insertion currently replaces an existing fragment with the same name.
  - Prefer preserving enough information to report duplicates rather than losing the earlier definition.

## LSP Diagnostics Publication

- Keep diagnostic publication generic.
  - After any open document changes, publish diagnostics for all open documents.
  - Do not add fragment-specific invalidation or publication rules in the LSP layer.
  - Let Picante/input tracking decide what needs real recomputation.

- Revisit publication batching once files scale.
  - The current MVP approach is O(open files) per edit.
  - That is acceptable while Picante caches unchanged work, but later we may want debounce/batching or request-side cancellation for large workspaces.

## Tests To Add

- Query in file B spreads fragment in file A, and planning/output includes the spread fields.
- Query in file B updates diagnostics immediately when fragment in file A changes.
- Fragment cycle produces a diagnostic and does not recurse indefinitely.
- Duplicate output key introduced by fragment spread is reported.
- Duplicate fragment name across two files is deterministic according to the chosen namespace model.
