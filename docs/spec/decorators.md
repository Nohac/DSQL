# Decorators

Status: RFC.

Decorators attach directives to inferred query metadata without declaring new
variables or changing the query's source shape. They exist for metadata that is
about generated params, generated input objects, generated result paths, table
views, form fields, cache policy, validation, or UI hints, but that would make a
query body noisy if written inline.

Decorators are not a second declaration system for variables. Variables are
still discovered from query usage, for example through `$$movie_id`,
`$$filters.title`, or directive expressions. A decorator can only target a path
that exists in the checked query metadata.

## Design Goals

- Keep query bodies usage-driven.
- Let validation, cache, table, and UI metadata live near the query without
  forcing explicit variable declarations.
- Support both nested and flat path syntax for generated metadata paths.
- Reuse the directive system for all decorator payloads.
- Report diagnostics when a decorator targets a missing query, variable, input,
  result, field, or table view path.
- Normalize decorators into checked metadata consumed by generation and editor
  stages.

## Syntax

A decorator declaration starts with `decorate`, names the metadata space, and
selects a query.

```dsql
decorate vars on MovieDetail {
  params {
    movie_id @zod.validate(schema: "MovieId")
  }

  input {
    include_cast @zod.validate(schema: "BooleanFlag")
  }
}
```

The initial metadata spaces are:

```text
vars
result
```

`decorate vars` targets generated variable/input metadata. `decorate result`
targets generated result metadata. Additional spaces may be added when the
compiler has a stable checked model for them.

Open question: whether the keyword should remain `decorate vars on QueryName`
or be shortened to `decorate QueryName`. The longer form makes the targeted
metadata space explicit and avoids looking like a query declaration.

## Variable Decoration

Variable decorators attach to inferred variable paths for a query.

```dsql
query MovieDetail {
  movies(where .id == $$movie_id) {
    id
    title
    cast @.include_if(if: $$input.include_cast) {
      actor {
        name
      }
    }
  }
}

decorate vars on MovieDetail {
  params {
    movie_id @zod.validate(schema: "MovieId")
  }

  input {
    include_cast @zod.validate(schema: "BooleanFlag")
  }
}
```

The decorator does not create `movie_id` or `include_cast`. If the query stops
using `$$movie_id` or `$$input.include_cast`, the decorator becomes invalid and
the compiler reports an unresolved decorator path diagnostic.

Variable roots:

```text
params
input
context
```

`params` targets path variables inferred from `$$name` or equivalent query
parameter usage. `input` targets generated input object paths, including nested
input used by generated filters, forms, or conditional directives. `context`
targets runtime context variables when such variables are part of the checked
query model.

## Flat Paths

Decorators support flat dot paths so deeply nested generated shapes do not
require large wrapper blocks.

```dsql
decorate vars on MovieSearch {
  input.movies.clause.where {
    title @zod.validate(schema: "MovieTitleSearch")
    released_after @zod.validate(schema: "Year")
  }
}
```

This is equivalent to the nested form:

```dsql
decorate vars on MovieSearch {
  input {
    movies {
      clause {
        where {
          title @zod.validate(schema: "MovieTitleSearch")
          released_after @zod.validate(schema: "Year")
        }
      }
    }
  }
}
```

A block path and child names concatenate. A directive attaches to the final path
segment.

```dsql
decorate vars on MovieSearch {
  input.movies.clause.where {
    title @zod.validate(schema: "MovieTitleSearch")
  }
}
```

The checked target path is:

```text
vars.input.movies.clause.where.title
```

Path segments use generated public metadata names, including aliases, not raw
catalog names unless those names are also the generated names.

## Result Decoration

Result decorators attach metadata to generated result paths.

```dsql
query MovieDetail {
  movies(where .id == $$movie_id) {
    id
    title
    director {
      name
    }
    stats {
      rating
    }
  }
}

decorate result on MovieDetail {
  movies @table.view(name: "movie_detail") {
    id @table.column(pinned: true)
    title @table.column(label: "Title", sortable: true)
    director.name @table.column(label: "Director")
    stats.rating @table.column(label: "Rating", sortable: true)
  }
}
```

The directives in this example are extension metadata. They do not alter SQL or
result shape unless a registered generator or provider consumes them.

Nested and flat result paths are equivalent:

```dsql
decorate result on MovieDetail {
  movies {
    director {
      name @table.column(label: "Director")
    }
  }
}
```

## Directive Semantics

Decorators use normal directive invocation syntax. Directive definitions declare
which decorator locations they support.

Proposed decorator directive locations:

```text
decorator_vars
decorator_params
decorator_input
decorator_context
decorator_result
decorator_result_object
decorator_result_field
```

The compiler validates both the directive location and the target path. A
directive that is valid on `scalar_selection` is not automatically valid on a
decorator result field unless its definition declares that location too.

Repeatability, source order, typed references, and provider capabilities follow
the directive spec.

## Checking

Decorator checking runs after query variables and result metadata have been
inferred for the scoped program.

Validation rules:

1. resolve the query named by `decorate ... on QueryName`;
2. resolve the decorator metadata space;
3. normalize nested and flat paths into canonical checked paths;
4. resolve each target path against the query's inferred metadata;
5. validate attached directive names, locations, arguments, typed references,
   variables, and repeatability;
6. emit checked decorator metadata for generation and editor stages.

Diagnostics should include:

- unknown decorated query;
- unsupported decorator metadata space;
- unresolved decorator path;
- decorator path resolves to the wrong kind of metadata;
- duplicate non-repeatable directive;
- directive not allowed on decorator location;
- invalid directive argument or typed reference.

## Interaction With Query Syntax

Inline directives and decorators can both target the same generated concept.
For example, a result field may have an inline `@.deprecated` directive and a
decorator-provided `@table.column` directive.

```dsql
query Movies {
  movies {
    old_title: title @.deprecated(reason: "Use title")
  }
}

decorate result on Movies {
  movies.old_title @table.column(label: "Old title", hidden: true)
}
```

When multiple directives target the same checked path, the compiler preserves
their source origins. Consumers should receive both inline checked directives
and decorator checked directives, tagged by origin.

Conflicts are directive-specific. The base language only enforces duplicate
non-repeatable directives and unsupported locations.

## Examples

### Conditional Field And Input Validation

```dsql
query MovieDetail {
  movies(where .id == $$movie_id) {
    id
    title
    cast @.include_if(if: $$input.include_cast) {
      actor {
        name
        profile {
          image_url
        }
      }
    }
  }
}

decorate vars on MovieDetail {
  params {
    movie_id @zod.validate(schema: "MovieId")
  }

  input {
    include_cast @zod.validate(schema: "BooleanFlag")
  }
}
```

### Cache Metadata

```dsql
decorate result on MovieDetail {
  movies @cache.ttl(seconds: 300, bust_query: MovieSearch)
}
```

The exact `@cache.ttl` schema is extension-owned. The directive can declare a
typed `query` reference for `bust_query`.

### Table Mapping

```dsql
decorate result on MovieDetail {
  movies @table.view(name: "movie_detail") {
    title @table.column(label: "Title")
    director.name @table.column(label: "Director")
    cast.actor.name @table.column(label: "Cast")
  }
}
```

## Open Questions

- Should `decorate vars` and `decorate result` be separate declarations or one
  declaration with named roots?
- Should decorator paths have a sigil to distinguish generated metadata paths
  from catalog paths, or is the decorator space enough context?
- Can a decorator apply to a fragment, or only to generated query artifacts?
- Should decorators be allowed in separate files imported by the query's
  analysis environment?
- How should a formatter group large flat-path decorator blocks?
