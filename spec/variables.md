# Variables

Status: unfinished.

Variables allow query input values to be inferred from their usage and bound at
execution or generation time.

## Intended Shape

Queries should not need explicit variable declarations. Variable names and types
are inferred from where variables appear.

```dsql
query KeywordDiscovery {
  keyword(where .movie_keyword.title.production_year > $ order by keyword asc limit $) {
    id
    keyword
    movie_keyword(limit $movie_limit) {
      title {
        id
        title
        production_year
        movie_info(limit $) {
          info
          note
          info_type {
            info
          }
        }
      }
    }
  }
}
```

`$` is an anonymous variable. Its input key is inferred from the field, clause,
or semantic role where it is used.

`$movie_limit` is a named variable. Its type is still inferred from the usage
site, but its input key is the explicit variable name.

## Inferred Input Shape

The compiler should emit a structured variable input schema that follows the
query shape and keeps clause inputs separate from nested body inputs.

Conceptual shape for the example above:

```json
{
  "keyword": {
    "clause": {
      "where": {
        "movie_keyword": {
          "title": {
            "production_year": "int"
          }
        }
      },
      "limit": "int"
    },
    "body": {
      "movie_keyword": {
        "clause": {
          "movie_limit": "int"
        },
        "body": {
          "title": {
            "body": {
              "movie_info": {
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
- `where .movie_keyword.title.production_year > $` becomes a nested
  `where.movie_keyword.title.production_year` input with the catalog type of
  `production_year`.
- `limit $` becomes a `limit` input with integer type.
- `offset $` becomes an `offset` input with integer type.

Named variables use the explicit variable name as the input key:

```dsql
movie_keyword(limit $movie_limit) {
  id
}
```

This produces a `movie_limit` input at that selection’s clause scope, typed as
an integer because it is used in `limit`.

The compiler must still preserve binding metadata that maps every generated
input key back to its usage site, clause role, source span, inferred type, and
SQL parameter position.

## Top-Level Params

Most variables should produce structured query input under `input`, but dsql
should also support explicit top-level params for generated routes, UI forms, and
manually shaped APIs.

```dsql
query MovieLookup {
  movie_info(where .id > $$id limit $$) {
    id
  }
}
```

`$$id` is a named top-level param. It maps to `params.id`.

`$$` is an anonymous top-level param. Its name is inferred from usage, but it is
still emitted under `params`, not under the structured `input` tree.

Conceptual generated input shape:

```json
{
  "params": {
    "id": "int",
    "limit": "int"
  },
  "input": {
    "movie_info": {
      "clause": {}
    }
  }
}
```

The four variable forms are:

```dsql
$        # structured anonymous inferred input
$name    # structured named input
$$       # top-level anonymous inferred param
$$name   # top-level named param
```

Top-level params still infer type from usage:

- `where .id > $$id` creates `params.id` with the catalog type of `.id`.
- `where .id > $$` creates `params.id`.
- `limit $$` creates `params.limit` with integer type.
- `offset $$` creates `params.offset` with integer type.
- `where .movie_keyword.title.production_year > $$` creates
  `params.production_year` by default.

Reusing a top-level param is allowed when every usage infers a compatible type
and semantic role.

```dsql
query MovieLookup {
  movie_info(where .id > $$id) {
    title(where .id == $$id) {
      id
    }
  }
}
```

If two anonymous top-level params infer the same name from different semantic
paths, the compiler should report a diagnostic rather than silently merge them.

```dsql
query AmbiguousIds {
  movie_info(where .id > $$ and .title.id > $$) {
    id
  }
}
```

Both anonymous params infer `id`, but they come from different paths. The user
should name them:

```dsql
query AmbiguousIds {
  movie_info(where .id > $$movie_info_id and .title.id > $$title_id) {
    id
  }
}
```

## Ambiguity

Anonymous variable inference must be deterministic. If two anonymous variables
would infer the same key in the same input object and cannot be merged safely,
the compiler should report a diagnostic and ask the user to name one or both
variables.

Example:

```dsql
query Movies {
  movie_info(where .id > $ and .id < $) {
    id
  }
}
```

This may be ambiguous if both variables infer `where.id`. The user can
disambiguate:

```dsql
query Movies {
  movie_info(where .id > $min_id and .id < $max_id) {
    id
  }
}
```

The compiler may eventually infer operator-qualified anonymous names such as
`id_gt` and `id_lt`, but the first implementation should prefer diagnostics
over surprising generated names.

## Operator Variables

Operator variables are a possible later layer for generated search/filter APIs.
They allow a closed set of operators to be selected by user input while keeping
the query structurally typed and safe.

```dsql
query MovieInfoSearch {
  movie_info(where .id $min_op[>, >=] $min_id) {
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
movie_info(where .id $[==, >, !=] $) {
  id
}
```

The anonymous operator key may infer as `id_op`, and the anonymous value key may
infer as `id`. If this would collide with another inferred key, the compiler
should ask the user to name one or both variables.

Conceptual public input shape:

```json
{
  "movie_info": {
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
      "path": "input.movie_info.clause.where.id.op",
      "values": [">", ">="],
      "controls": "predicate:movie_info.id"
    }
  ],
  "variants": {
    ">": "where movie_info.id > $1",
    ">=": "where movie_info.id >= $1"
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
query MovieInfoRange {
  movie_info(where .id $min_op[>, >=] $min_id $range_mode[and, or] .id $max_op[<, <=] $max_id) {
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
  "movie_info": {
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
  "movie_info": {
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
where .movie_keyword.title.production_year > $year
```

Hover on `$year`:

```text
input.keyword.clause.where.movie_keyword.title.production_year.year
type: int
role: where value
```

Anonymous structured variable example:

```dsql
limit $
```

Hover on `$`:

```text
input.keyword.clause.limit
type: int
role: limit
```

Top-level param example:

```dsql
limit $$limit
```

Hover on `$$limit`:

```text
params.limit
type: int
role: limit
```

Anonymous top-level param example:

```dsql
limit $$
```

Hover on `$$`:

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
input.movie_info.clause.where.id.id_op
type: enum(">", ">=")
role: comparison operator
field: movie_info.id
```

Anonymous operator variable example:

```dsql
where .id $[==, !=] $
```

Hover on `$[==, !=]`:

```text
input.movie_info.clause.where.id.op
type: enum("==", "!=")
role: comparison operator
field: movie_info.id
```

Boolean operator variable example:

```dsql
where .id $min_op[>, >=] $min_id $range_mode[and, or] .id $max_op[<, <=] $max_id
```

Hover on `$range_mode[and, or]`:

```text
input.movie_info.clause.where.id.range_mode
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

Compound values may be useful later:

```dsql
[1, 2, 3]
{ id: 1 }
```

Open questions:

- Whether variable defaults allow only literals or richer expressions.
- How variable nullability should be represented.
- Whether compound values belong in the query language or only in filter/input
  positions.
- How provider-specific scalar types are named.
- Whether operator-qualified anonymous names should be inferred for repeated
  comparisons against the same field.
- Whether boolean operator variables are worth supporting, or whether generated
  APIs should model those cases as explicit higher-level filter objects.
- Whether `params` should allow defaults or whether defaults only belong in the
  generated host/API layer.

User values must be emitted as SQL parameters when this is implemented.
