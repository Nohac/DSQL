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
documents = ["queries/shared/**/*.dsql"]

[resolution.frontend]
documents = ["src/**/*.tsx", "queries/frontend/**/*.dsql"]
imports = ["shared"]

[resolution.api]
documents = ["queries/api/**/*.dsql"]
imports = ["shared"]
```

Each loaded document belongs to exactly one scope. If the same DSQL file or
embedded region is matched by more than one scope, project loading should report
a deterministic ownership error.

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
