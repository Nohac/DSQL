# Resolution Scopes

Status: in progress.

Resolution scopes let one project contain multiple independent DSQL surfaces
that share the same catalog but resolve queries, fragments, filters, and
conditions through separate definition maps.

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

`documents = []` is valid when a scope imports another scope, allowing an
output-only consumer such as `shared_output` below. A scope with neither
documents nor imports is a configuration diagnostic because it has no source
or effective definitions.

## Resolver Model

Each scope has local definitions and an effective resolver.

Local definitions are the queries, fragments, filters, and conditions owned
directly by that scope. The effective resolver contains the local definitions
plus definitions imported from other scopes, copied by value. Importing does
not create a separate runtime surface dependency.

Filters and conditions are standalone-only definitions. An embedding resolver
that extracts either form reports a diagnostic because it has no host-language
runtime value to substitute. Imported filters participate in the importing
scope's operation analysis and match lock even when no query names them
explicitly.

Rules:

- The same definition name may exist in different independent scopes.
- Duplicate local names inside one scope are diagnostics.
- A local definition that collides with an imported definition is a diagnostic.
- Two imported scopes that provide the same definition name to one importing
  scope are a diagnostic.
- Unknown imports and cyclic imports are diagnostics.
- Imported shared definitions are emitted into each reachable terminal
  generation target so outputs are self-contained.

Fragment lookup and query planning use the current effective resolver. LSP,
CLI validation, generation, completion, hover, and checking should all consume
the same resolver semantics instead of rebuilding scope rules independently.

## Terminal Generation Targets

Not every resolution scope is a generation target.

A **terminal generation target** is a configured scope that is not imported by
any other configured scope. Equivalently, it has no dependents in the
dependency-to-consumer graph. The implicit `default` scope is terminal.

For the configuration above, `frontend` and `api` are generation targets while
`shared` is not. Their effective artifact surfaces are:

```text
frontend = frontend + shared
api      = api + shared
```

A non-terminal scope is reusable compiler input. Its standalone queries and
fragments are copied into every reachable terminal target's effective closure,
but the scope is not rendered as an independent deployable surface. If the same
shared surface also needs independent output, the project declares another
terminal scope that imports it:

```toml
[resolution.shared_output]
documents = []
imports = ["shared"]
```

A derived embedded DSQL document may belong only to a terminal generation
target. An embedded query or fragment has one host callsite that must rewrite to
one generated module; a non-terminal scope may feed multiple self-contained
target outputs and therefore has no unique rewrite target. Extracting an
embedded DSQL region in a non-terminal scope is a deterministic project error.
A resolver-matched host containing no DSQL regions remains valid and produces
nothing. Reusable non-terminal definitions live in standalone `.dsql`
documents.

The term *terminal generation target* avoids the ambiguous word *leaf*: imports
are commonly drawn from consumer to dependency, in which orientation a shared
dependency rather than a consumer would appear leaf-like.

## Generation Metadata

The compiler should expose enough scope metadata for host integrations:

- every resolution scope, its direct imports, and whether it is a terminal
  generation target
- source-file ownership entries for embedded and standalone documents
- each scope's effective operation and fragment artifact group
- effective filter and condition provenance used by each scope

Renderers dispatch only groups marked as terminal generation targets. Each
target group includes the target's complete effective closure, including
artifacts declared by non-terminal dependencies.

Vite and other embedding transforms use source-file ownership to select the
right generated query barrel for a transformed file. Because embedding hosts
are terminal-only, each callsite has exactly one target render map. In
multi-target projects, host generators return render metadata for every
terminal target that owns embedded DSQL.
