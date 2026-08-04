# Variables

Status: implemented for inferred scalar and collection variables, defaults,
nullability, fragment bindings, bounded dynamic input capability presets, and
explicit trusted-context declarations. The deferred extensions called out
below remain unfinished.

Variables allow query input values to be inferred from their usage and bound at
execution or generation time.

## Intended Shape

Queries should not need explicit public-input declarations. Public variable
names and types are inferred from where variables appear. Trusted host context
is declared separately because declaration-by-use would make misspellings look
like new context inputs.

```dsql
query UserSearch {
  users(where .posts.comments.created_at > $ order by name asc limit $) {
    id
    name
    posts(limit $post_limit) {
      comments {
        id
        body
        created_at
        attachments(limit $) {
          id
          url
        }
      }
    }
  }
}
```

`$` is an anonymous variable. Its input key is inferred from the field, clause,
or semantic role where it is used.

`$post_limit` is a named variable. Its type is still inferred from the usage
site, but its input key is the explicit variable name.

## Inferred Input Shape

The compiler should emit a structured variable input schema that follows the
query shape and keeps clause inputs separate from nested body inputs.

Conceptual shape for the example above:

```json
{
  "users": {
    "clause": {
      "where": {
        "posts": {
          "comments": {
            "created_at": "timestamptz"
          }
        }
      },
      "limit": "int"
    },
    "body": {
      "posts": {
        "clause": {
          "post_limit": "int"
        },
        "body": {
          "comments": {
            "body": {
              "attachments": {
                "clause": {
                  "limit": "int"
                }
              }
            }
          }
        }
      }
    }
  }
}
```

The schema format above is illustrative. Generated targets may use TypeScript,
JSON Schema, OpenAPI, or another host representation, but the semantic structure
should be stable.

## Inference Rules

Anonymous variables infer their key and type from their usage:

- `where .id == $` becomes a `where.id` input with the catalog type of `.id`.
- `where .posts.comments.created_at > $` becomes a nested
  `where.posts.comments.created_at` input with the catalog type of `created_at`.
- `limit $` becomes a `limit` input with integer type.
- `offset $` becomes an `offset` input with integer type.

Named variables use the explicit variable name as the input key:

```dsql
posts(limit $post_limit) {
  id
}
```

This produces a `post_limit` input at that selection’s clause scope, typed as
an integer because it is used in `limit`.

The compiler must still preserve binding metadata that maps every generated
input key back to its usage site, clause role, source span, inferred type, and
SQL parameter position.

## Top-Level Params

Most variables should produce structured query input under `input`, but dsql
should also support explicit top-level params for generated routes, UI forms, and
manually shaped APIs.

```dsql
query UserLookup {
  users(where .id > %id limit %) {
    id
  }
}
```

`%id` is a named top-level param. It maps to `params.id`.

`%` is an anonymous top-level param. Its name is inferred from usage, but it is
still emitted under `params`, not under the structured `input` tree.

Conceptual generated input shape:

```json
{
  "params": {
    "id": "int",
    "limit": "int"
  },
  "input": {
    "users": {
      "clause": {}
    }
  }
}
```

The four variable forms are:

```dsql
$        # structured anonymous inferred input
$name    # structured named input
%       # top-level anonymous inferred param
%name   # top-level named param
```

Host-provided global context uses a separate form:

```dsql
$:name   # host-provided context value
```

Context values are not public query inputs. They are declared explicitly in
standalone DSQL source and supplied by a trusted server-side adapter or request
boundary. A generated client cannot bind or override them. Execution refuses
to start when required context is missing.

### Trusted-Context Declarations

A scope declares its trusted context in one or more scope-level blocks:

```dsql
context {
  user_id: uuid
  tenant_id: uuid
  tenant_ids: uuid[]
  is_admin: boolean
  status: public::account_status
}
```

Each entry declares one context name and its authoritative logical type. A type
may be an unqualified built-in logical type or a schema-qualified
catalog/provider type. Catalog/provider types must be qualified; the compiler
does not choose between same-named types from different schemas. `T[]` declares
a collection of `T`; the collection suffix is part of context declaration
syntax in this contract and does not extend structural filter or condition
target shapes.

Context blocks are ordinary standalone DSQL definitions for visibility
purposes, but their entries are the names resolved by the effective scope.
Normal resolution-scope collision rules apply to those entry names verbatim:
duplicate local entries, a local entry colliding with an imported entry, and
two imports supplying the same entry are diagnostics. Independent scopes may
declare the same entry name. Context blocks are not allowed in embedded host
regions because they produce no host-language value.

Every `$:name` usage must resolve to exactly one visible entry. The compiler
does not create a declaration from a use, infer a context type from an
expression, or fall back to project configuration or provider metadata for a
missing declaration. The declared type is checked against every use, including
scalar versus collection shape, boolean-only positions, and the nominal
identity of catalog enums or other provider types. For example,
`.tenant_id in $:tenant_ids` requires `tenant_ids` to be declared as a
collection compatible with the logical type of `.tenant_id`.

The initial declaration contract is required and non-null. Query source cannot
mark context optional or give it a default. Trusted-context defaults, if later
supported, remain a server/provider-boundary concern rather than a way for a
query to weaken an enforcement requirement.

Context remains operation-global rather than fragment-bound. A fragment use is
carried unchanged into the consuming operation, and generated metadata contains
only the visible context entries actually used by that operation and its
effective fragment, filter, and condition closure.

Completion after the context sigil offers visible declared entry names. Hover
shows the declaration's logical type and trusted requiredness. Goto-definition
targets the declaring entry, including an imported declaration. An undeclared,
ambiguous, or type-incompatible use is a language diagnostic.

This declaration syntax and behavior are implemented.

Top-level params still infer type from usage:

- `where .id > %id` creates `params.id` with the catalog type of `.id`.
- `where .id > %` creates `params.id`.
- `limit %` creates `params.limit` with integer type.
- `offset %` creates `params.offset` with integer type.
- `filter SoftDelete when %` creates boolean `params.soft_delete`, inferred by
  converting the filter name to the normal generated lower-snake-case form.
- `where .posts.comments.created_at > %` creates `params.created_at` by
  default.

Reusing a top-level param is allowed when every usage infers a compatible type
and semantic role.

```dsql
query UserLookup {
  users(where .id > %id) {
    posts(where .author.id == %id) {
      id
    }
  }
}
```

If two anonymous top-level params infer the same name from different semantic
paths, the compiler should report a diagnostic rather than silently merge them.

```dsql
query AmbiguousIds {
  users(where .id > % and .posts.author.id > %) {
    id
  }
}
```

Both anonymous params infer `id`, but they come from different paths. The user
should name them:

```dsql
query AmbiguousIds {
  users(where .id > %user_id and .posts.author.id > %author_id) {
    id
  }
}
```

## Definition Input Refinements

Required, non-null inputs remain inferred from their usage. A query or fragment
header lists only inputs whose inferred contract is refined with nullability or
a default:

```dsql
query RecentMovies(
  %limit? = null
  $cast_limit = 5
) {
  titles(limit %) {
    cast(limit $cast_limit) {
      name
    }
  }
}

fragment RecentPosts(
  $created_after? = null
  %limit = 5
) on users {
  posts(where .created_at > $created_after limit %) {
    id
  }
}
```

The header is not a complete variable declaration list. An entry must carry
`?`, `=`, or both; listing an otherwise ordinary required, non-null input is
redundant and a diagnostic. Query headers may mix input refinements with
operation-wide `filter` assignments. Fragment headers contain input refinements
only. Directives follow the closing parenthesis as usual.

Conceptually:

```text
query_header_item = input_refinement | filter_assignment
fragment_header_item = input_refinement
input_refinement = ("$" | "%") Name ("?" ["=" default_value] | "=" default_value)
```

The name in a refinement identifies the generated binding, regardless of
whether its body occurrence was named explicitly. For example, `limit %`
infers `params.limit`; completion after `%` in the query header offers
`limit`, and `%limit? = null` refines that anonymous occurrence. The same
applies to a structured anonymous `$` occurrence and its inferred key.

Header completion offers every inferred binding not already refined, including
its generated path, type, role, and source location. A `%name` entry identifies
the compatible top-level param at `params.name` and may cover compatible
repeated usages. A `$name` entry must identify exactly one structured input
path. If multiple structured paths infer the same key, the diagnostic and
completion details name every candidate; the author must name the body
occurrences distinctly before refining them. An entry with no matching inferred
binding is a diagnostic.

Bare `$` and `%` remain valid anonymous usage-site variables, but are not
valid refinement entries because a header needs a stable inferred key. In a
definition-reference binding list, those bare sigils instead denote complete
input roots. Trusted `$:name` context cannot be refined by query source;
trusted-context defaults, if ever supported, belong to the server/provider
boundary so a query cannot weaken an enforcement requirement.

### Requiredness And Nullability

Requiredness and nullability are independent:

| Source contract | Required | Nullable | Omission |
| --- | ---: | ---: | --- |
| no header entry | yes | no | error |
| `%value = 10` | no | no | substitute `10` |
| `%value?` | yes | yes | error |
| `%value? = 10` | no | yes | substitute `10` |
| `%value? = null` | no | yes | substitute `null` |

The same matrix applies to `$name`. There is no optional-without-a-default
state: optionality means omission has a deterministic replacement value.
`null` is valid as a default only for a `?` input, and a non-null default does
not remove the caller's ability to provide `null` when `?` is present.

The first implementation accepts compile-time scalar and homogeneous collection
literals. Defaults cannot reference fields, other public variables, trusted
context, functions, or row-dependent expressions. Bounded dynamic predicates
and ordering additionally admit their empty identity values as described in
[Dynamic Input Defaults](#dynamic-input-defaults). Rich object defaults remain
deferred until general object-literal syntax is designed.

Collection defaults cannot contain `null` elements until element nullability is
represented explicitly through metadata, materialization, and SQL generation.
This restriction applies only to definition defaults: query-authored membership
lists and caller-supplied collection values retain PostgreSQL-compatible null
elements. A nullable collection may itself use `null` as its whole default.
Defaults for `limit` and `offset` must be non-negative integers in the supported
`int` range; an invalid default is a diagnostic and cannot remove a pagination
clause during planning.

Defaults are type-checked by the compiler and serialized into generated
metadata. At execution, materialization substitutes a missing value before
ordinary input validation and SQL parameter binding. A supplied value always
wins. One definition header is the authoritative default for a logical binding;
defaults are not repeated at usage sites.

### Nullable Predicate Uses

When a nullable public variable supplies an operand to an ordinary comparison,
membership test, or other predicate atom, a runtime `null` makes that complete
atom absent. It does not bind SQL `NULL`, become `IS NULL`, or change the
operator:

```dsql
query Movies(
  %from? = null
  %to? = null
) {
  titles(
    where .kind_id == 1
      and (
        .production_year >= %from
        or .production_year <= %to
      )
  ) {
    id
  }
}
```

Operand order does not change this rule. If one atom contains multiple nullable
public operands, the atom is present only when every controlling operand is
non-null; any null operand removes the complete atom before boolean-tree
pruning.

Predicate absence is structural pruning, not replacement with boolean `true`:

```text
A and absent -> A
absent and B -> B
A or absent  -> A
absent or B  -> B
not absent   -> absent
```

Parentheses collapse with their contents. If the complete `where` expression
becomes absent, the selection has no query-authored predicate. This definition
keeps optional comparisons correct beneath both `and` and `or`; blindly
lowering each absent atom to `true` would make an `or` branch match every row.

Only the public input controls presence. With a non-null input, a nullable
database column still follows PostgreSQL three-valued comparison semantics.
Matching database null remains explicit through `is null` or `is not null`.
For membership, a null collection removes the atom while `in []` remains an
active predicate that is false. Enforced filters and trusted-context predicates
cannot be pruned by nullable public inputs.

Other nullable roles use their structural identities: null removes a `limit` or
`offset` clause, an optional order item contributes no ordering, and a nullable
bounded predicate or order input contributes no dynamic entries. Empty dynamic
predicate and order values remain the preferred non-null representations.
Filter-assignment `when` conditions must be non-null booleans; refining such an
input with `?` is a diagnostic rather than introducing a third assignment state.

The semantic IR records conditional predicate presence. SQL backends may use
compiler-produced variants or a correctly guarded single statement, but must
preserve tree-pruning semantics. A nullable input never participates in a
unique-predicate cardinality proof, even with a non-null default, because the
caller may explicitly provide null. A non-null input with a non-null default
may participate because its predicate is always present.

## Ambiguity

Anonymous variable inference must be deterministic. If two anonymous variables
would infer the same key in the same input object and cannot be merged safely,
the compiler should report a diagnostic and ask the user to name one or both
variables.

Example:

```dsql
query Users {
  users(where .id > $ and .id < $) {
    id
  }
}
```

This may be ambiguous if both variables infer `where.id`. The user can
disambiguate:

```dsql
query Users {
  users(where .id > $min_id and .id < $max_id) {
    id
  }
}
```

The compiler may eventually infer operator-qualified anonymous names such as
`id_gt` and `id_lt`, but the first implementation should prefer diagnostics
over surprising generated names.

## Flattened And Transformed Selections

Variable inference follows semantic selection scopes, while flattening changes
only result shape.

A keyed relation or aggregate selection keeps its ordinary output-key path:

```dsql
query Users {
  users {
    recent_stats: posts(where .created_at >= $since) | aggregate {
      count
    }
  }
}
```

Conceptually, `$since` remains under the `recent_stats` selection's clause
scope. The alias distinguishes it from another independently filtered `posts`
selection.

A flattened selection has no output wrapper. Its variables use the underlying
root or relation scope, with a transform segment when needed to distinguish the
source clause from an ordinary body selection:

```dsql
query Users {
  users {
    ...posts(where .created_at >= $since) | aggregate {
      post_count: count
    }
  }
}
```

Conceptual input path:

```text
input.users.body.posts.aggregate.clause.where.since
```

Flattening must not make one generated input silently control two distinct
selection instances. Two flattened selections over the same semantic relation,
or a flattened selection beside a keyed selection using the same inferred path,
produce an ambiguity diagnostic when their inferred inputs cannot be merged
safely. Naming variables may disambiguate their input keys; an implementation
must not use source order or result flattening as hidden identity.

The same rule applies at query roots. A root aggregate's clauses belong to the
root source scope whether its aggregate object is keyed or flattened into the
query result.

## Bound Definition Inputs

Some source constructs reference another definition that has its own inferred
variables. Fragment spreads are the first example, but the same model should
also apply to query references in directives, split-fetch handles, and future
definition-like language constructs.

The target definition infers its own `$` structured inputs and `%` top-level
params from its body and refines that contract in its definition header. A
reference may leave those roots contained, bind individual leaves, flatten a
complete root into the caller, or place a complete root beneath a caller
namespace. These are all checked path transformations over one input contract,
not separate fragment-argument and query-argument systems.

References in this section to retaining bounded dynamic surfaces describe the
eventual fragment extension. The initial bounded-dynamic slice rejects every
dynamic input owned by a fragment, so none of those paths can occur until the
extension in
[Bounded Dynamic Predicates And Ordering](#bounded-dynamic-predicates-and-ordering)
is implemented.

Conceptually, a binding list contains one or more variable references or
mappings:

```text
binding_list = "(" binding_item (","? binding_item)* [","] ")"
binding_item = variable_ref ["<-" variable_ref]
variable_ref = ("$" | "%") [Name]
```

Items may be separated by whitespace or commas, and a trailing comma is valid.
Trusted `$:name` context is global and is not part of this binding syntax.

### Default Fragment Containment

A fragment spread without a binding list keeps the fragment's inferred variables
contained under the spread path.

```dsql
fragment UserPanel(
  $created_after? = null
  %limit = 10
) on users {
  posts(where .created_at > $created_after limit %) {
    id
  }
}

query Users {
  users {
    ...UserPanel
  }
}
```

The generated input shape is implementation-defined, but conceptually the
fragment variables stay under the `UserPanel` spread namespace:

```text
input.users.UserPanel.input.posts.created_after
input.users.UserPanel.params.limit
```

This is valid without explicit bindings. The fragment is a reusable source
definition and the spread site owns a contained instance of its input contract.
The contained fields retain the fragment's requiredness, nullability, defaults,
and bounded dynamic surfaces. Each spread instance may override an optional
contained field independently; omission uses the fragment default.

### Explicit Bindings

A named leaf binding supplies one target input from an independently inferred
caller input:

```dsql
query Users(
  %page_size = 20
) {
  users(where .created_at > $after limit %page_size) {
    ...UserPanel(
      $created_after <- $after,
      %limit <- %page_size,
    )
  }
}
```

The left side of `<-` identifies a variable inferred by the target definition,
including an anonymous occurrence through its inferred name. The right side is
a variable occurrence in the caller context. It participates in the caller's
normal inference and definition-header refinement rules exactly as if it
appeared in a clause at that reference site, using the target variable's type
and role as its inference context.

An explicit leaf binding replaces the target leaf's default and requiredness
with the caller source contract. In the example, omission materializes the
query's `20` and forwards it; the fragment's `10` is no longer consulted. With
no query refinement, `%page_size` is required and non-null. This follows
ordinary function-call behavior: omitting an argument uses the callee default,
while explicitly supplying an argument delegates its value contract to the
caller expression.

Explicit mappings may cross variable roots when the inferred types are
compatible:

```dsql
...UserPanel(
  $inner_input <- %outer_param,
  %inner_param <- $outer_input,
)
```

This maps a target structured input from a caller top-level param and a target
top-level param from a caller structured input. The generated caller paths come
from the source variables, not the target variables.

Source and target nullability remain directional. A non-null caller may bind a
nullable target. A nullable caller cannot bind a non-null target, even when the
caller has a non-null default, because it still admits explicit null. An
optional non-null caller with a non-null default may satisfy a required non-null
target because materialization always supplies a value.

### Named Forwarding Shorthand

A named binding item may omit `<-` when the target and source generated names
are the same:

```dsql
...UserPanel($created_after, %limit)
```

This is equivalent to forwarding target `$created_after` from caller
`$created_after`, and target `%limit` from caller `%limit`.

### Whole-Root Lifting

Bare `$` and `%` in a binding list are root operators, not requests to guess
one anonymous leaf:

```dsql
...UserPanel($, %)
```

A bare `$` flattens the target's complete structured-input root into the caller
at the spread path. A bare `%` flattens the complete target params root into
the caller params root. Every leaf keeps the fragment's inferred name and path,
type, role, requiredness, nullability, default, bounded dynamic surface, and
conditional-access metadata. Anonymous occurrences in the fragment already
have stable inferred keys, so no per-leaf guessing is necessary.

For `UserPanel`, full lifting produces conceptually:

```text
params.limit
input.users.posts.created_after
```

Lifted leaves merge with existing caller bindings only when their complete
contracts are compatible. Defaults, nullability, dynamic capability surfaces,
and roles participate in compatibility. Two identical lifted contracts may
merge; different defaults or surfaces are conflicts rather than sources for an
arbitrary precedence rule.

### Namespaced Root Lifting

A complete target root may instead be remapped beneath a named caller object:

```dsql
...SearchableUser(
  $ <- %searchable_user_input
  % <- %searchable_user_params
)
```

The target sigil selects the fragment root. The source sigil selects the caller
root, and its name becomes a path prefix. Assuming the fragment has
`input.posts.created_after`, `params.limit`, and `params.pattern`, the mapping is:

```text
input.posts.created_after -> params.searchable_user_input.posts.created_after
params.limit              -> params.searchable_user_params.limit
params.pattern            -> params.searchable_user_params.pattern
```

This remains leaf metadata plus path-prefix transformation; it does not require
an untyped object parameter or runtime reinterpretation. Cross-root namespace
bindings are valid, so either target root may map beneath a named `$` structured
object or `%` top-level object.

The namespace object is required when any contained leaf remains required. If
every leaf has a default, the namespace is optional and omission behaves as an
empty object so the leaf defaults apply. A namespace retains inner defaults,
nullability, dynamic surfaces, and provenance exactly like direct root lifting.
The namespace object itself is non-null: explicit `null` is invalid even when
the object is optional through all-defaulted leaves.

Whole-root binding copies a target contract; named leaf binding supplies an
independent caller value. That distinction determines whether target defaults
are preserved.

### Per-Root Binding Mode

Binding mode is activated independently for each target root:

```text
$    structured input root
%   top-level params root
```

If a binding list mentions any `$` target binding, every required target
structured input must be covered by a root binding or named leaf binding.
Defaulted target leaves may be omitted and then use their fragment defaults.
They do not remain contained after their root enters explicit binding mode.
Unbound target `%` params remain contained unless the list also mentions a
`%` target binding.

The same rule applies symmetrically to `%`: every required target param must be
covered, defaulted target params may be omitted and use their defaults, and an
unmentioned `$` root remains contained.

For example:

```dsql
...UserPanel(%)
```

The complete target params root is flattened. The target structured input root
remains contained:

```text
params.limit
input.users.UserPanel.input.posts.created_after
```

And:

```dsql
...UserPanel($)
```

The complete target structured input root is flattened. The target params root
remains contained:

```text
input.users.posts.created_after
input.users.UserPanel.params.limit
```

This avoids forcing users to bind both roots while preventing accidental
half-lifting within one root. The initial language rejects mixing a whole-root
binding with named leaf bindings for that same target root; override/exclusion
syntax can be added later if concrete use cases justify it.

### Binding Diagnostics

Within an explicitly bound root:

- every required target variable for that root must be covered;
- omitted defaulted targets use their definition defaults;
- a target variable may be bound at most once;
- binding a variable that the target definition does not infer is a diagnostic;
- leaf source and target types, collection shapes, roles, and nullability must
  be directionally compatible;
- root merges require compatible complete contracts, including defaults and
  dynamic surfaces;
- a namespace source must not collide with an incompatible scalar, dynamic
  input, or differently shaped namespace;
- source variables merge into the caller using the same ambiguity rules as
  ordinary variable usage.

Missing target variables are diagnostics at the reference site only for roots
that are in explicit binding mode. Roots not mentioned in the binding list keep
their default contained input shape.

### Bound Query References

The same binding model applies when a directive or future metadata feature
references a query with inferred inputs.

```dsql
query UserCard @cache(
  invalidated_by: [
    {
      query: UserCardCacheKey(%id <- %user_id),
      keys: [.updated_at],
    },
  ],
) {
  users(where .id == %user_id) {
    id
    name
  }
}

query UserCardCacheKey {
  users(where .id == %id) {
    id
    updated_at
  }
}
```

The cache directive does not define variables itself. It references a query and
uses the shared binding model to map the referenced query's inferred variables
from the current query's inferred variables. Context values are not bound here:
`$:name` values are global host context and are supplied by the runtime through
the normal context mechanism.

## Bounded Dynamic Predicates And Ordering

Some generated API surfaces need programmatic, type-safe filtering and ordering
where the exact field choice is made by application code. This should not use an
unrestricted `where %` placeholder. A raw predicate placeholder hides the
capability surface, makes relationship depth unclear, and can turn a fixed DSQL
query into a broad dynamic query builder.

Instead, dynamic predicates and order inputs are bounded by the DSQL document.
DSQL provides four compiler-owned capability presets:

```dsql
query Fields(
  %search = {}
  %indexed_search = {}
  %order = []
  %indexed_order = []
) {
  fields(
    where .context_id == %context_id
      and %search on selected
      and %indexed_search on indexed
    order by %order on selected,
      %indexed_order on indexed
    limit %limit
    offset %offset
  ) {
    id
    name
    display
    created_at
  }
}
```

The `on` token disambiguates a bounded dynamic input from an ordinary variable.
Without it, a variable in predicate position is a boolean value:

```dsql
where %enabled
```

`on` is reserved from `variable_name` in every variable position: `%on`,
`$on`, fragment bindings named `on`, and operator variables named `on` are
invalid. The preset names are contextual rather than reserved:
`$selected`, `%indexed`, `$selected_indexed`, and `$searchable` are ordinary
variable names when they do not follow `on`. The other currently admitted
keywords remain valid variable names:
`query`, `fragment`, `where`, `order`, `by`, `limit`, `offset`, `asc`, `desc`,
and the contextual keywords `not`, `in`, `is`, `exists`, `filter`, `condition`,
`apply`, `when`, and `field`. A sigil immediately followed by `on` and a preset
therefore starts an anonymous dynamic input; that shape is diagnosed because
bounded dynamic inputs must be named.

The initial contract has these boundaries:

- only named top-level `%name` inputs are accepted;
- the containing definition must be a query;
- `selected`, `indexed`, `selected_indexed`, and `searchable` are accepted;
- every preset is shallow;
- anonymous dynamic inputs, fragment-local dynamic inputs, explicit allowlists,
  project-defined presets, and deep relation traversal are diagnostics or
  deferred syntax rather than partially supported behavior.

Keeping dynamic inputs on query definitions makes every public capability
surface explicit in one generated operation. Fragment lifting may extend the
same contract later, but the compiler rejects dynamic inputs inside a fragment
instead of silently containing or widening them.

A dynamic predicate usage must be the complete predicate or occur in positive
conjunctive position beneath `and`. A usage beneath `or` or `not` is a
diagnostic. The public dynamic object may still contain its own recursive `or`
and `not` nodes. This restriction lets an empty whole input
behave as structural absence without replacing an `or` operand with `TRUE`;
supporting arbitrary source-level boolean placement requires a later
presence-aware SQL plan.

The predicate root is the `where` clause owned by the selection. A dynamic
usage inside an `exists` source's nested `where` clause is a diagnostic because
that nested source does not declare a public dynamic input surface.

### Capability Presets

All four presets expand at compile time into a normalized list of public keys,
catalog columns, logical types, predicate operators or order directions,
nullability, and conditional-access metadata:

- `selected` contains scalar projections directly present in the effective
  selection body after fragment expansion. A projection alias is its public
  key.
- `indexed` contains visible source columns independently addressable by a
  usable physical index. It uses catalog column names and does not require a
  row selection body.
- `selected_indexed` is the intersection of `selected` and `indexed`. It keeps
  selected aliases and therefore requires a row selection body.
- `searchable` contains visible text-like source columns independently
  addressable by a physical index key whose metadata advertises the `like`
  capability. It uses catalog column names and does not require a row selection
  body.

For ordered index families such as `btree` and unknown/custom access methods,
only the leading key is independently addressable. For `gin`, `gist`, `spgist`,
`brin`, and `hash`, each true key is independently addressable. Included
columns are never preset fields. These names describe stable catalog capability
classes; they do not promise that every supplied runtime value or compound
predicate will avoid a scan.

`selected` and `selected_indexed` are invalid on an aggregate source because an
aggregate has no row selection body. `indexed` and `searchable` remain valid in
an aggregate source clause list:

```dsql
query SearchSummary(
  %search = {}
  %indexed = {}
) {
  users(
    where %search on searchable
      and %indexed on indexed
  ) | aggregate {
    count
  }
}
```

A preset that expands to no fields is a diagnostic rather than a generated
empty type. A dynamic usage inside an `exists` source remains invalid because
that nested source does not declare a public dynamic input surface.

`%search on selected` is one predicate atom. It means:

- `%search` is a top-level generated parameter.
- the allowed predicate fields are scalar fields selected directly in the
  effective current selection body after fragment expansion;
- the resolved scalar must belong to the current collection source, so
  flattening a relationship field does not make that relationship path part of
  the shallow surface;
- a selected alias is the public dynamic field name, while metadata retains the
  resolved catalog field identity;
- available operators are compiler-owned and derived from each resolved field's
  logical type;
- nested relation fields are not included by default.

The catalog-backed presets apply the same rules, but expose source column names
instead of selection aliases. If a catalog-backed name collides with `and`,
`or`, or `not`, the query author must select and alias that column and use
`selected` or `selected_indexed`.

Selections that resolve to the same public key must already satisfy the normal
result-shape collision rules. A reused named dynamic input is valid only when
every usage has the same preset spelling, normalized kind, and capability
contract: public field keys, resolved logical types, operators or directions,
nullability, and conditional-access metadata. Two presets that happen to
expand identically are still distinct authored contracts. Otherwise the
compiler requires separate names.

Conceptual generated predicate input:

```ts
type FieldsSearch = {
  and?: FieldsSearch[];
  or?: FieldsSearch[];
  not?: FieldsSearch;
  id?: {
    eq?: string;
    neq?: string;
    in?: string[];
    not_in?: string[];
    is_null?: boolean;
  };
  name?: {
    eq?: string;
    neq?: string;
    like?: string;
    in?: string[];
    not_in?: string[];
    is_null?: boolean;
  };
  display?: {
    eq?: string;
    neq?: string;
    like?: string;
    in?: string[];
    not_in?: string[];
    is_null?: boolean;
  };
  created_at?: {
    eq?: string;
    neq?: string;
    gt?: string;
    gte?: string;
    lt?: string;
    lte?: string;
    in?: string[];
    not_in?: string[];
    is_null?: boolean;
  };
};
```

The exact operator set is type-aware:

- `eq`, `neq`, `in`, `not_in`, and `is_null` apply to comparable scalar types;
- `gt`, `gte`, `lt`, and `lte` apply to ordered scalar types;
- `like` applies to text-like scalar types.

The compiler owns both these public operator names and their SQL lowering.
Runtimes consume compiler metadata; they must not maintain a parallel
type-to-operator table or interpolate user-provided SQL syntax.

Predicate objects compose recursively. Sibling field entries, sibling operators
within one field, and `and` alongside other entries are combined with `AND`.
`or` combines its members with `OR`; `not` negates its nested predicate.
Object key order has no semantic effect.

```ts
const search: FieldsSearch = {
  name: { like: "A%", neq: "Archived" },
  or: [
    { display: { like: "%featured%" } },
    { created_at: { gte: "2025-01-01T00:00:00Z" } },
  ],
};
```

Dynamic predicate objects use the same structural-pruning algebra as nullable
static predicates. An object with no active entries and an empty field-operator
object are absent. Absent members are removed from `and` and `or`; `not` of an
absent child is absent. An `and` with no active members is absent rather than an
active `TRUE`, because lowering absence to `TRUE` would make a containing `or`
branch match every row. An explicitly supplied `or` with no active members is
the active false predicate.

Examples:

```text
{ or: [{}, X] }          -> X
{ or: [{}] }             -> false
{ and: [] }              -> absent
{ not: {} }              -> absent
{ not: { or: [] } }      -> true
```

The last form negates an active false predicate and is therefore an active true
predicate. It can intentionally make a containing dynamic `or` match every row,
just as `not_in: []` can. Dynamic filters constrain application-selected rows;
they are not an authorization boundary and never remove enforced static
filters.

Runtime values follow the same logical wire validation as ordinary inputs.
`null` is not accepted as an operand for `eq`, `neq`, ordered comparisons,
`like`, `in`, or `not_in`, and `in`/`not_in` collections may not contain null
elements. Null tests use `is_null: true` and `is_null: false`, lowering to
`IS NULL` and `IS NOT NULL`. Empty membership preserves the static predicate
contract: `in: []` is false and `not_in: []` is true.

This intentionally differs from nullable static operands, whose null value
prunes their complete predicate atom. A dynamic caller expresses absence by
omitting an operator key; nullable null is accepted only for the complete
dynamic input and maps to its empty identity.

`order by %order on <preset>` means:

- `%order` is a top-level generated parameter.
- the allowed order fields are the same shallow field set exposed by that
  preset;
- the input is an array, and its element order is SQL precedence order;
- each array element must contain exactly one field;
- directions are `asc`, `desc`, `asc_nulls_first`, `asc_nulls_last`,
  `desc_nulls_first`, and `desc_nulls_last`.

Conceptual generated order input:

```ts
type FieldsOrder = Array<
  | { id: FieldsOrderDirection }
  | { name: FieldsOrderDirection }
  | { display: FieldsOrderDirection }
  | { created_at: FieldsOrderDirection }
>;

type FieldsOrderDirection =
  | "asc"
  | "desc"
  | "asc_nulls_first"
  | "asc_nulls_last"
  | "desc_nulls_first"
  | "desc_nulls_last";
```

Repeated fields are accepted and retain their supplied positions, matching
ordinary SQL ordering even when an entry is redundant. A future static syntax
for explicit null placement should use the same six compiler-owned direction
semantics.

Static and dynamic order items may coexist, and one clause may contain multiple
dynamic order inputs. Each dynamic array expands in place, so its internal order
and the precedence of surrounding static or dynamic items are both preserved.
The typed constant identity occupies that same position without affecting row
order.

The main design rule is that dynamic inputs expose a static capability surface.
The query author chooses which fields and relationship paths are available, and
the generated API only allows those choices.

Dynamic predicates and ordering operate on the same filtered logical fields as
static query clauses. A conditionally hidden scalar behaves as `NULL`, and a
conditionally hidden relation behaves as absent. Generated metadata must mark
dynamic fields whose readable value is conditional so UI and server tooling can
describe the surface accurately, but a dynamic input never bypasses a filter.
Consequently, `is_null` observes the same policy-filtered value as static
`is null`: on a row-dependent hidden field it may match rows where the readable
value was masked to `NULL`. This is intentional parity, not access to the raw
column.

### Dynamic Input Defaults

Definition headers refine bounded dynamic inputs by their inferred name just as
they refine scalar bindings:

```dsql
query Fields(
  %search = {}
  %order = []
  %limit = 50
  %offset = 0
) {
  fields(
    where .context_id == %context_id
      and %search on selected
    order by %order on selected
    limit %limit
    offset %offset
  ) {
    id
    name
    display
    created_at
  }
}
```

The usage site remains the sole authority for the bounded capability surface.
The header default cannot add a field, operator, relation path, or ordering mode
that the normalized `on` surface does not expose. Defaults are validated only
after selection expansion.

The canonical initial defaults are algebraic identity values:

- `{}` is an empty dynamic predicate and contributes no predicate atom;
- `[]` is an empty dynamic order and contributes no order entries;
- an empty `and` collection is structurally absent;
- an empty `or` collection is an active false predicate.

Only empty `{}` is admitted before general object-literal defaults are designed.
Rich predicate objects and non-empty order lists are a later additive extension.
This still makes the common generated API optional without adding nullable
types: `search?: FieldsSearch` defaults to `{}`, and `order?: FieldsOrder`
defaults to `[]`.

If a dynamic binding is explicitly refined with `?`, runtime null contributes
the same structural absence as its identity value. This applies to the complete
dynamic input, not to its individual field operands. `{}` and `[]` remain the
preferred defaults because they avoid equivalent omitted/null/empty states in
generated clients and cache keys.

Omitted defaults are materialized before validation and execution. Cache keys
use the materialized dynamic input, recursively sort object keys, and preserve
array order. In particular, order arrays retain precedence, and predicate
composition arrays retain their supplied traversal order. Omitted, defaulted,
and nullable-null identity values therefore share one canonical cache identity.

### SQL And Runtime Contract

The compiler emits one collision-safe marker for every dynamic predicate or
order usage site in both formatted and compact SQL. Marker allocation must prove
that the marker is absent from both complete generated forms before using it.
Replacement is literal, not regular-expression based and not JavaScript
replacement-string based.

Dynamic predicate identity replaces its whole marker with a compiler-owned true
expression. Dynamic order identity replaces its whole marker with a
compiler-owned typed constant order expression that has no effect on row order.
The latter must be integration-tested against Postgres; emitting an empty string
after `ORDER BY` is invalid.

The runtime traverses each materialized dynamic input once in deterministic
order. Dynamic operand values become ordinary SQL parameters appended after all
fixed parameters. When one named input is used at multiple compatible sites,
the runtime reuses the same allocated parameter positions while rendering each
site's compiler-owned readable field expressions. It never places a caller
string into SQL.

The dynamic input object is never itself a positional SQL parameter. Its
runtime-appended operands are not entries in `sql.parameters`, whose paths
resolve to declared ordinary fields. Each dynamic operand takes its logical type
and binding rules from the compiler-owned capability field/operator record.

Compiler metadata owns:

- the public field key and resolved catalog identity;
- logical type, nullability, and conditional-access classification;
- allowed public operators or directions;
- the SQL lowering for every allowed operator at every site;
- usage-site markers and readable SQL field expressions;
- the canonical empty default for its dynamic input kind;

The browser operation object carries only the ordinary public input/default
contract needed for typing, default materialization, and cache keys. Expanded
capabilities, SQL expressions, and usage sites belong to the server execution
payload and full generated artifact. This prevents browser bundles from
receiving server-only catalog and policy detail while keeping renderers and API
generators fully informed.

Dynamic inputs are ordinary public input fields with a full path such as
`params.search`, a `dynamic_predicate` or `dynamic_order` logical type, and their
normal required, nullable, and default flags. The capability side table is keyed
by that full path rather than by an unqualified name.

Runtime validation rejects unknown fields, unknown operators or directions,
malformed recursive nodes, scalar/collection shape mismatches, null operands,
multi-field order entries, and values outside the ordinary logical wire
contract before database execution.

### Deferred Extensions

The following are useful additive extensions, but are outside the initial
feature:

- explicit surfaces such as `on { selected .task.name like }`;
- project-defined presets;
- explicit or automatic deep relation traversal;
- rich non-empty object/list defaults;
- fragment-local dynamic inputs and fragment binding/lifting;
- anonymous dynamic inputs;
- static query syntax for explicit null ordering.

Future presets remain normalized capability expansions, never runtime shortcuts.
Deep traversal must remain explicit enough that a query author can review the
exposed relationship surface.

## Codegen Notes

Variables should produce a metadata contract that generated clients and endpoint
adapters can use without reinterpreting the query.

Conceptual full artifact shape:

```json
{
  "params": [
    {
      "path": "params.search",
      "data_type": "dynamic_predicate",
      "required": false,
      "default": { "kind": "empty_object" }
    }
  ],
  "context": [
    {
      "path": "context.tenant_id",
      "data_type": "uuid",
      "required": true
    }
  ],
  "dynamic_inputs": [
    {
      "path": "params.search",
      "kind": "predicate",
      "surface": "selected",
      "fields": [
        {
          "key": "name",
          "catalog_path": "public.fields.name",
          "data_type": "text",
          "nullable": false,
          "access": "unconditional",
          "operators": ["eq", "neq", "like", "in", "not_in", "is_null"]
        }
      ],
      "sites": [
        {
          "marker": "<compiler-owned marker>",
          "fields": [
            {
              "key": "name",
              "expression": "<compiler-owned readable SQL expression>",
              "operators": ["<compiler-owned SQL lowerings>"]
            }
          ]
        }
      ]
    }
  ]
}
```

`$:<name>` context values appear as required trusted host context, not public
query variables. Only a server-side adapter or request boundary may bind them.
Generated browser clients do not expose context setters or merge context into
the public operation input.

The exact serialized operator-lowering records may use templates or structured
prefix/suffix fields, but they must be compiler-produced data that cannot contain
caller-selected SQL syntax. Dynamic predicate and order inputs expose their
expanded field/operator surface so application code can generate type-safe UI
controls without knowing DSQL internals.

## Operator Variables

Operator variables are a possible later layer for generated search/filter APIs.
They allow a closed set of operators to be selected by user input while keeping
the query structurally typed and safe.

```dsql
query UserSearch {
  users(where .id $min_op[>, >=] $min_id) {
    id
  }
}
```

`$min_op[>, >=]` is an operator variable:

- It is only valid in operator position.
- Its allowed operators are explicitly enumerated.
- Every allowed operator must be valid for the left-hand field type.
- The compiler should lower it as a closed conditional SQL plan. User input must
  never become arbitrary SQL syntax.

`$[*]` should not be supported. It is too close to an "any operator" escape
hatch, makes generated APIs less explicit, and weakens type-aware validation.
If shorthand is needed later, it should be named and type-aware, such as
`$[comparison]` or `$[text_match]`, and it should expand to a compiler-defined
operator set.

Anonymous operator variables are possible:

```dsql
users(where .id $[==, >, !=] $) {
  id
}
```

The anonymous operator key may infer as `id_op`, and the anonymous value key may
infer as `id`. If this would collide with another inferred key, the compiler
should ask the user to name one or both variables.

Conceptual public input shape:

```json
{
  "users": {
    "clause": {
      "where": {
        "id": {
          "op": "== | > | !=",
          "value": "int"
        }
      }
    }
  }
}
```

## Dynamic Operator Lowering

Postgres does not allow a normal SQL parameter to stand in for an operator.
This is invalid:

```sql
where id $1 $2
```

Operator variables therefore cannot lower to ordinary SQL placeholders. The
current implementation target should be **SQL variants**:

```dsql
where .id $op[>, >=] $id
```

Conceptual generated metadata:

```json
{
  "operators": [
    {
      "path": "input.users.clause.where.id.op",
      "values": [">", ">="],
      "controls": "predicate:users.id"
    }
  ],
  "variants": {
    ">": "where users.id > $1",
    ">=": "where users.id >= $1"
  }
}
```

The next layer, such as a JS framework, Rust runtime, stored route generator, or
API adapter, chooses one compiler-produced SQL variant from the closed enum and
then binds values as normal SQL parameters. This keeps DSQL responsible for
parsing, validation, SQL generation, and metadata, while host integrations can
enrich the result without constructing SQL syntax from user strings.

The safe host-side rule is:

- Selecting from compiler-produced SQL variants is allowed.
- Passing user values as SQL parameters is required.
- Interpolating arbitrary user strings as raw SQL operators is forbidden.

Tagged-template SQL libraries may support trusted raw SQL fragments, but DSQL
metadata should not require consumers to call `raw(user_input)`. If a framework
uses raw fragments internally, they must be selected from compiler-generated
allowlist branches.

## Requires More Consideration

### Value-Operator SQL

A stored procedure or very generic runtime may prefer a single SQL statement
where the operator remains a value:

```sql
where
  ($1 = '>' and id > $2)
  or ($1 = '>=' and id >= $2)
```

This is injection-safe because the operator input is data, not SQL syntax. It is
also friendlier to stored procedures because no template interpolation is
required.

Tradeoffs:

- The generated SQL is less readable than concrete variants.
- The planner may have less opportunity to optimize each concrete predicate.
- Multiple dynamic operators can expand the SQL significantly.
- It may be the best fit for single-statement or stored-procedure targets.

This should remain a backend strategy to consider later, not the first lowering
target.

### Dynamic SQL In Stored Procedures

Stored procedures could also use dynamic SQL with `EXECUTE`, but that moves SQL
syntax construction into the database and requires strict allowlist handling.
This does not fit the default DSQL goal of producing readable, normal SQL.

### Backend Strategy

The semantic IR should preserve operator variables as conditional predicates so
backends can choose a lowering strategy:

```text
DynamicComparison {
  field,
  op_input,
  allowed_ops,
  value_input,
}
```

Initial target:

- `variants`: emit one SQL branch per allowed operator.

Future possible target:

- `value_operator`: emit one SQL statement with value-driven boolean expansion.

## Boolean Operator Variables

Boolean operator variables are the same idea applied between complete predicate
expressions.

```dsql
query UserRange {
  users(where .id $min_op[>, >=] $min_id $range_mode[and, or] .id $max_op[<, <=] $max_id) {
    id
  }
}
```

Rules:

- Boolean operator variables are only valid between complete predicate
  expressions.
- They must use an explicit allowlist. The initial useful set is `[and, or]`.
- They do not make precedence dynamic. Parentheses and the query AST still
  define expression structure.
- Generated SQL must switch over a closed enum and must not interpolate raw
  boolean operator text.

Conceptual internal shape:

```json
{
  "users": {
    "clause": {
      "where": {
        "left": {
          "field": "id",
          "op": "min_op",
          "value": "min_id"
        },
        "combine": "range_mode",
        "right": {
          "field": "id",
          "op": "max_op",
          "value": "max_id"
        }
      }
    }
  }
}
```

Generated public input may flatten this when it is ergonomic:

```json
{
  "users": {
    "clause": {
      "where": {
        "id": {
          "min_op": "> | >=",
          "min_id": "int",
          "range_mode": "and | or",
          "max_op": "< | <=",
          "max_id": "int"
        }
      }
    }
  }
}
```

Operator variables and boolean operator variables should not be part of the
first scalar-variable implementation. They should be added after basic value
variables, input schema generation, type inference, and parameter binding are
stable.

## Tooling

Hover on a variable should show the generated input binding, inferred type, and
source role. The displayed path should match the generated schema shape so users
can understand the API/codegen contract from the editor.

Structured variable example:

```dsql
where .posts.comments.created_at > $created_after
```

Hover on `$created_after`:

```text
input.users.clause.where.posts.comments.created_at.created_after
type: timestamptz
role: where value
```

Anonymous structured variable example:

```dsql
limit $
```

Hover on `$`:

```text
input.users.clause.limit
type: int
role: limit
```

Top-level param example:

```dsql
limit %limit
```

Hover on `%limit`:

```text
params.limit
type: int
role: limit
```

Anonymous top-level param example:

```dsql
limit %
```

Hover on `%`:

```text
params.limit
type: int
role: limit
```

Operator variable example:

```dsql
where .id $id_op[>, >=] $id
```

Hover on `$id_op[>, >=]` or `$id_op`:

```text
input.users.clause.where.id.id_op
type: enum(">", ">=")
role: comparison operator
field: users.id
```

Anonymous operator variable example:

```dsql
where .id $[==, !=] $
```

Hover on `$[==, !=]`:

```text
input.users.clause.where.id.op
type: enum("==", "!=")
role: comparison operator
field: users.id
```

Boolean operator variable example:

```dsql
where .id $min_op[>, >=] $min_id $range_mode[and, or] .id $max_op[<, <=] $max_id
```

Hover on `$range_mode[and, or]`:

```text
input.users.clause.where.id.range_mode
type: enum("and", "or")
role: boolean operator
```

Diagnostics for collisions or incompatible variable reuse should reference the
same generated input path shown by hover.

## Values To Consider

Variables are first needed as scalar values in clause positions:

```dsql
$
$name
```

Typed list values are also required for `in` and `not in` predicates:

```dsql
[1, 2, 3]
```

The field use infers the collection element type for a literal or variable.
Membership collections permit `null` elements even when the compared field is
not nullable, matching PostgreSQL array and `IN` behavior. Generated collection
inputs therefore admit `null` elements and preserve the three-valued semantics
defined by [Membership](query.md#membership).

Object values and general collection expressions may be useful later:

```dsql
{ id: 1 }
```

The empty object `{}` is already reserved as the identity default for a bounded
dynamic predicate. That narrow header-only use does not imply general object
expressions or rich object defaults.

Open questions:

- Whether object values and general collection expressions belong in the query
  language or only in filter/input positions, and which rich values should
  eventually be valid defaults.
- Whether SQL generation should prefer a bounded set of statement variants or
  guarded expressions for structurally optional predicate trees.
- How provider-specific scalar types are named.
- Whether operator-qualified anonymous names should be inferred for repeated
  comparisons against the same field.
- Whether boolean operator variables are worth supporting, or whether generated
  APIs should model those cases as explicit higher-level filter objects.

User values must be emitted as SQL parameters when this is implemented.
