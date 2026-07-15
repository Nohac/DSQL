# Resolution Scopes

Status: in progress.

Resolution scopes let one project contain multiple independent DSQL surfaces
that share the same catalog but resolve queries and fragments through separate
definition maps.

This is a project configuration feature, not language-level namespacing. DSQL
source continues to use plain query and fragment names. There is no syntax such
as `api::MovieFields`.

## Configuration

Projects without explicit resolution configuration have one implicit `default`
scope containing every loaded DSQL document and embedded DSQL region.

Projects can define named scopes:

```toml
[resolution.shared]
documents = [{ resolver = "dsql", paths = ["queries/shared/**/*.dsql"] }]

[resolution.frontend]
documents = [
  { resolver = "dsql", paths = ["queries/frontend/**/*.dsql"] },
  { resolver = "typescript", paths = ["src/**/*.ts", "src/**/*.tsx"] },
]
imports = ["shared"]

[resolution.api]
documents = [{ resolver = "dsql", paths = ["queries/api/**/*.dsql"] }]
imports = ["shared"]
```

Each document entry selects physical files and the resolver that turns each file
into DSQL documents. The built-in `dsql` resolver treats the whole file as one
document. Other names select an `[embedding.<resolver>]` extraction provider;
`typescript` defaults to the tagged-template regex when no section overrides it.
Regex is the current provider strategy, not part of source ownership: a future
tree-sitter provider can use the same resolver-bearing document entries.

```toml
[embedding.vue]
strategy = "regex"
pattern = 'dsql`(?P<content>[\s\S]*?)`'

[resolution.frontend]
documents = [{ resolver = "vue", paths = ["src/**/*.vue"] }]
```

Extensions have no compiler-defined meaning. A resolver may select any file
name, and files outside every configured path are not project inputs. An LSP
buffer outside the project set is analyzed only when the client explicitly
opens it with `languageId = "dsql"`; unmatched host files have no extractor and
are ignored.

Glob paths are recommended. A bare directory intentionally assigns every file
below it to that resolver, including editor files or documentation, so use one
only when the directory is resolver-homogeneous.

Each physical file has exactly one `(scope, resolver)` assignment. Overlap
between scopes, or between two resolvers in one scope, is a deterministic
ownership error in cold loading and forces equivalent reconciliation in a warm
daemon.

## Resolver Model

Each scope has local definitions and an effective resolver.

Local definitions are the queries and fragments owned directly by that scope.
The effective resolver contains the local definitions plus definitions imported
from other scopes, copied by value. Importing does not create a separate runtime
surface dependency.

Rules:

- The same query or fragment name may exist in different independent scopes.
- Duplicate local names inside one scope are diagnostics.
- A local definition that collides with an imported definition is a diagnostic.
- Two imported scopes that provide the same definition name to one importing
  scope are a diagnostic.
- Unknown imports and cyclic imports are diagnostics.
- Imported shared definitions are emitted into each importing generated surface
  so outputs are self-contained.

Fragment lookup and query planning use the current effective resolver. LSP,
CLI validation, generation, completion, hover, and checking should all consume
the same resolver semantics instead of rebuilding scope rules independently.

## Generation Metadata

The compiler should expose enough scope metadata for host integrations:

- the list of generated scopes and their imports
- source-file ownership entries for embedded and standalone documents
- per-scope operation and fragment artifact groups

Vite and other embedding transforms use source-file ownership to select the
right generated query barrel for a transformed file. In multi-scope projects,
host generators must return render metadata for each generated scope that can
own embedded DSQL.
