# Directives

Status: RFC.

Directives attach schema-defined metadata or behavior to DSQL source constructs.
They are the extension point for language-owned behavior, provider integration,
code generation hints, validation metadata, and editor-aware annotations.

Directives must be structured. They are not textual macros and must not rewrite
source before parsing. Any directive that affects checking, planning, SQL,
generation, or editor behavior must be represented as typed syntax and handled
by the compiler.

## Design Goals

- System directives and extension directives use the same declaration model.
- Directive declarations are described with JSON Schema plus DSQL-specific
  metadata for locations and typed references.
- Directives declare where they are valid, similar to GraphQL directive
  locations.
- Directive arguments can refer to catalog-aware concepts such as tables,
  columns, relation fields, selected fields, or output paths.
- Those typed arguments should power validation, completion, hover, and
  generated metadata.
- DSQL-owned directives should be explicit, namespaced, and easy to read.
- Extension directives should be namespaced so project and package features do
  not collide.

## Directive Names

Directive names are qualified names after `@`.

```dsql
@dsql.include_if
@zod.validate
@tanstack.query_options
```

The `dsql` namespace is reserved for language-owned directives.

DSQL-owned directives may use the short namespace form `@.`:

```dsql
@.include_if
```

This is equivalent to:

```dsql
@dsql.include_if
```

The short form exists because system directives are expected to be common in
query documents, and `@.` keeps them visually distinct from third-party
extensions while avoiding a long prefix.

Bare directive names are reserved for future use and should not be accepted in
new syntax:

```dsql
@include_if   # invalid; use @.include_if or @dsql.include_if
```

Extension directives must use a namespace:

```dsql
@zod.validate
@openapi.operation
@my_app.form_field
```

The first segment identifies the provider or project extension. Later segments
identify the directive within that provider.

## Directive Invocation Syntax

A directive invocation starts with `@`, followed by a directive name, followed
by an optional argument list.

```dsql
@zod.validate(schema: "User")
@.include_if(if: $include_posts)
@ui.column(label: "Email", hidden: false)
```

Directive arguments are named. Positional directive arguments are not part of
the language.

```dsql
@zod.validate("User") # invalid
```

The argument list uses DSQL expression values plus typed references:

```dsql
@example.directive(
  string_value: "hello",
  number_value: 123,
  boolean_value: true,
  null_value: null,
  variable_value: $enabled,
  column_ref: .email,
  table_ref: public.users,
)
```

Open question: object and array literals are useful for extension directives,
but they overlap with JSON expression support that is not fully specified yet.
The first implementation may restrict directive values to scalars, variables,
and typed references.

## Directive Locations

Directive declarations specify the source locations where the directive is
valid.

Initial directive locations:

```text
document
query
fragment
root_selection
field_selection
relation_selection
scalar_selection
fragment_spread
clause_list
where_clause
order_by_clause
limit_clause
offset_clause
```

Example:

```json
{
  "x-dsql-directive": {
    "name": "example.directive",
    "locations": ["field_selection", "fragment_spread"]
  }
}
```

Location names describe the checked language context, not just the CST rule.
For example, `scalar_selection` and `relation_selection` are both syntactically
field selections, but they differ after catalog resolution.

This lets a directive declare precise placement:

```json
{
  "x-dsql-directive": {
    "name": "example.scalar_only",
    "locations": ["scalar_selection"]
  }
}
```

The compiler should report a diagnostic when a directive is used at an
unsupported location.

## Directive Definitions

All directives are defined by directive definition documents. A definition uses
JSON Schema for its argument object and DSQL-specific extension fields for
language metadata.

Conceptual shape:

```json
{
  "$id": "https://dsql.dev/directives/dsql/include_if.schema.json",
  "title": "dsql.include_if",
  "type": "object",
  "required": ["if"],
  "properties": {
    "if": {
      "type": "boolean",
      "x-dsql-expression": true
    }
  },
  "additionalProperties": false,
  "x-dsql-directive": {
    "name": "dsql.include_if",
    "locations": ["field_selection", "fragment_spread"],
    "effects": ["result_shape", "generation_input"]
  }
}
```

The JSON value validated by the schema is the directive argument object.

For:

```dsql
@zod.validate(schema: "User")
```

the schema input is:

```json
{
  "schema": "User"
}
```

Directive definitions should use ordinary JSON Schema keywords wherever
possible:

- `type`
- `required`
- `properties`
- `additionalProperties`
- `enum`
- `oneOf`
- `anyOf`
- `allOf`
- `default`
- `description`

DSQL-specific behavior belongs under `x-dsql-*` extension keys.

## System Directives

System directives are owned by the DSQL language and live in the `dsql`
namespace. They may be written with `@dsql.` or the short `@.` form.

System directives are compiled by DSQL itself. If a system directive affects
checking, planning, SQL, generated metadata, variables, or editor behavior, that
effect must be implemented in the compiler.

Example:

```dsql
query Users {
  users {
    id
    posts @.include_if(if: $include_posts) {
      id
      title
    }
  }
}
```

`@.include_if` is only an example until conditional result semantics are fully
specified.

System directives should not be configurable by arbitrary project schemas.
Projects may choose whether to enable language features through compiler
configuration, but they cannot redefine `dsql.include_if`.

## Extension Directives

Extension directives are supplied by project configuration, provider packages,
or adapter integrations.

Example:

```dsql
query CreateUserForm {
  users {
    email @zod.validate(schema: "EmailAddress")
    name @ui.field(label: "Display name")
  }
}
```

Extension directives must declare their effects. Initial effect categories:

```text
metadata
validation
generation
editor
plan
sql
result_shape
generation_input
runtime
```

Most extension directives should be metadata-only:

```json
{
  "x-dsql-directive": {
    "name": "zod.validate",
    "locations": ["scalar_selection"],
    "effects": ["metadata", "generation"]
  }
}
```

Extension directives that claim `plan`, `sql`, or `result_shape` effects require
a compiler/provider implementation. A schema alone cannot change query
semantics.

Unknown extension directives should be diagnostics by default. A project may
allow preserving unknown extension directives as metadata, but that mode should
be explicit because misspelled directive names otherwise become silent bugs.

## Typed Directive Arguments

JSON Schema validates primitive argument shape. DSQL-specific schema extensions
validate language-aware argument values and drive editor support.

Typed references are source-level values, but they normalize into structured
checked values before metadata or generation consume them. The JSON Schema
`type` still describes the public argument shape; `x-dsql-ref` tells the
compiler to parse, complete, resolve, and validate the argument as a DSQL
reference instead of as an arbitrary string.

Typed reference kinds:

```text
table
column
relation
field
field_selector
selected_field
output_path
fragment
query
directive
```

### `table`

A catalog table reference.

```json
{
  "type": "string",
  "x-dsql-ref": "table"
}
```

Accepted syntax:

```dsql
@example.table(target: public.users)
@example.table(target: users)
```

The compiler validates the table against the catalog using normal table
resolution rules. Completion should suggest visible catalog tables.

### `column`

A column on a table. The schema may specify where the table context comes from.

```json
{
  "type": "string",
  "x-dsql-ref": "column",
  "x-dsql-table": "current"
}
```

Accepted syntax inside a selection over `users`:

```dsql
@example.sortable(by: .email)
```

`x-dsql-table: "current"` means the reference resolves against the current
selection table. Other possible table scopes:

```text
current
root
parent
argument
```

`argument` means another directive argument supplies the table:

```json
{
  "properties": {
    "table": {
      "type": "string",
      "x-dsql-ref": "table"
    },
    "column": {
      "type": "string",
      "x-dsql-ref": "column",
      "x-dsql-table": { "argument": "table" }
    }
  }
}
```

### `relation`

A relation field from the current table to another table.

```json
{
  "type": "string",
  "x-dsql-ref": "relation",
  "x-dsql-table": "current"
}
```

Accepted syntax:

```dsql
@example.prefetch(relation: posts)
@example.prefetch(relation: public.posts::user_id)
```

Completion should use the same relation resolution as normal field selections.

### `field`

A field in the current field-selection context. This may be a scalar column,
relation, computed field, or future catalog-backed field.

```json
{
  "type": "string",
  "x-dsql-ref": "field",
  "x-dsql-table": "current"
}
```

Use this when the directive accepts any selectable field.

### `field_selector`

A relation selector such as the part after `::`.

```json
{
  "type": "string",
  "x-dsql-ref": "field_selector",
  "x-dsql-relation": { "argument": "relation" }
}
```

This is useful when a directive wants a relation target and selector separately
instead of accepting the combined relation syntax.

### `selected_field`

A field that is already selected in the current selection set.

```json
{
  "type": "string",
  "x-dsql-ref": "selected_field"
}
```

Accepted syntax:

```dsql
users @ui.table(default_sort: selected.email) {
  id
  email
}
```

Open question: whether `selected.email` should be syntax, a string, or a normal
path-like reference. The important semantic distinction is that the referenced
field must exist in the selected result shape, not merely in the catalog.

### `output_path`

A path in the generated result shape.

```json
{
  "type": "string",
  "x-dsql-ref": "output_path"
}
```

Output paths use aliases and generated output keys, not necessarily catalog
field names.

This is useful for generated UI metadata:

```dsql
query Users {
  users @ui.table(title: output.name) {
    id
    name
  }
}
```

Open question: whether output path literals need a dedicated sigil to avoid
confusion with predicate paths.

### `fragment`

A visible fragment name.

```json
{
  "type": "string",
  "x-dsql-ref": "fragment"
}
```

The compiler validates the name against the scoped program, so resolution
imports apply in the same way they do for fragment spreads.

### `query`

A visible query name in the current generation context.

```json
{
  "type": "string",
  "x-dsql-ref": "query"
}
```

This is mostly for generation metadata, not query execution.

## Reference Syntax

Directive arguments may use ordinary literals or typed references.

Examples:

```dsql
@example.table(target: public.users)
@example.column(target: .email)
@example.relation(target: posts)
@example.fragment(target: UserFields)
```

Whether an unquoted name is a string or a typed reference depends on the
directive schema. If the argument schema has `x-dsql-ref`, the compiler resolves
the value as a typed reference. Otherwise a bare name is invalid and the user
should use a string literal.

```dsql
@zod.validate(schema: "EmailAddress") # string
@example.fragment(target: UserFields) # fragment reference
```

This avoids stringly typed references while keeping string values explicit.

## Variables In Directives

Directive schemas may allow variables by setting `x-dsql-expression: true`.

```json
{
  "properties": {
    "if": {
      "type": "boolean",
      "x-dsql-expression": true
    }
  }
}
```

This allows:

```dsql
@.include_if(if: $include_posts)
```

Variables used in directives participate in normal variable inference. The
directive definition must declare the expected type and variable role so
generated inputs are stable.

Conceptual extension:

```json
{
  "type": "boolean",
  "x-dsql-expression": true,
  "x-dsql-variable-role": "directive_condition"
}
```

If a directive argument does not allow expressions, variables are invalid.

## Schema Registration

Directive definitions can come from:

- built-in DSQL system directive definitions;
- project-local directive schema files;
- provider packages;
- generator or adapter integrations.

A project should be able to register extension directives in config.

Conceptual shape:

```toml
[[directives]]
namespace = "zod"
schema = "dsql/directives/zod.schema.json"

[[directives]]
namespace = "ui"
schema = "dsql/directives/ui.schema.json"
```

Open question: whether schemas register one directive per file, many directives
per file, or both.

The normalized compiler input should be a directive registry:

```text
DirectiveRegistry {
  definitions: Map<DirectiveName, DirectiveDefinition>
}
```

The directive registry should be immutable tracked input so registry changes
invalidate checking, editor completion, generation metadata, and any directive
effect stages.

## Validation Rules

The compiler validates directives in this order:

1. Parse directive syntax.
2. Normalize directive names, including `@.` to `@dsql.`.
3. Resolve the directive definition from the registry.
4. Validate the directive location.
5. Validate argument names, required arguments, and primitive JSON Schema shape.
6. Resolve typed references using the directive schema and current semantic
   context.
7. Validate variable usage and infer directive variables when allowed.
8. Apply system or provider effects for directives that are not metadata-only.

Diagnostics should include:

- unknown directive;
- unknown directive namespace;
- directive not allowed at location;
- missing required argument;
- unknown argument;
- invalid argument type;
- invalid typed reference;
- unresolved typed reference;
- duplicate directive when a directive is declared non-repeatable;
- unsupported directive effect for the current compiler/provider.

## Repeatability

Directives are non-repeatable by default at the same location.

The directive definition may opt into repeatability:

```json
{
  "x-dsql-directive": {
    "name": "tag.label",
    "locations": ["field_selection"],
    "repeatable": true
  }
}
```

If a non-repeatable directive appears twice on the same construct, the compiler
reports a diagnostic.

## Effects

Directive effects describe which stages must account for the directive.

Initial effects:

```text
metadata
validation
generation
editor
plan
sql
result_shape
generation_input
runtime
```

Effect meanings:

- `metadata`: preserved in compiler/generation metadata.
- `validation`: produces semantic diagnostics.
- `generation`: affects generated code but not query semantics.
- `editor`: affects completion, hover, semantic tokens, or editor metadata.
- `plan`: changes the execution plan.
- `sql`: changes generated SQL.
- `result_shape`: changes result fields, nullability, or conditionality.
- `generation_input`: changes generated params/input/context types.
- `runtime`: requires host/runtime enforcement.

System directives with non-metadata effects must be implemented by DSQL.

Extension directives with non-metadata effects must have a provider capability
registered. A JSON Schema definition alone only gives validation, typed
references, completion, and metadata preservation.

## Metadata Preservation

Metadata directives should be preserved in checked compiler outputs in a
structured form.

Conceptual shape:

```text
CheckedDirective {
  name: DirectiveName,
  namespace: String,
  location: DirectiveLocation,
  arguments: Map<String, CheckedDirectiveValue>,
  effects: Vec<DirectiveEffect>,
  source_range: TextRange,
}
```

Generation should receive checked directives, not raw AST directives. This
prevents generators from revalidating directive names, argument types, and
catalog references.

Metadata-only extension directives should not affect SQL or result shape unless
a generator chooses to interpret their checked metadata.

## Editor Support

Directive definitions should power editor behavior:

- after `@`, complete available directive names for the current location;
- after `@.`, complete DSQL system directives;
- after `@namespace.`, complete directives in that namespace;
- inside argument lists, complete valid argument names;
- for typed reference arguments, complete catalog tables, columns, relations,
  fragments, output paths, or selected fields according to `x-dsql-ref`;
- hover on directive names should show schema `description`;
- hover on directive arguments should show schema property `description`;
- semantic tokens should classify directive names and typed references.

Completion must use the selected resolution environment and current catalog
snapshot.

## Formatting

The formatter may normalize directive spacing:

```dsql
field @ui.column(label: "Name")
```

Long directive argument lists may be split:

```dsql
field @ui.column(
  label: "Display name",
  hidden: false,
)
```

Formatting must remain CST-backed and conservative. If directive argument
syntax is malformed, the formatter should preserve the original source or avoid
rewriting the malformed directive.

## Initial System Directive Candidates

These are candidates, not accepted directives.

### `@dsql.include_if` / `@.include_if`

Conditionally includes a selection or fragment spread.

```dsql
posts @.include_if(if: $include_posts) {
  id
}
```

Open questions:

- Whether excluded singular relations are `null` or omitted.
- Whether excluded collection relations are `[]`, `null`, or omitted.
- How conditional selections affect TypeScript result types.
- Whether this belongs in core DSQL or an execution/generation profile.

### `@dsql.deprecated` / `@.deprecated`

Marks a query, fragment, field selection, or generated output path as
deprecated metadata.

```dsql
old_name: name @.deprecated(reason: "Use display_name")
```

This is metadata/editor/generation-only and should not affect SQL.

## Examples

### System Conditional Directive

```dsql
query Users {
  users {
    id
    name
    posts @.include_if(if: $include_posts) {
      id
      title
    }
  }
}
```

### Zod Validation Metadata

```dsql
query UserForm {
  users {
    email @zod.validate(schema: "EmailAddress")
    age @zod.validate(schema: "AdultAge")
  }
}
```

### Catalog-Aware Extension Directive

```dsql
query Users {
  users @ui.table(default_sort: .email) {
    id
    email @ui.column(label: "Email")
  }
}
```

The `default_sort` argument can declare `x-dsql-ref: "column"` with
`x-dsql-table: "current"`, so completion and validation use the `users` table.

## Open Questions

- Should directive argument lists support object and array literals immediately?
- Should output-path and selected-field references have dedicated syntax?
- Should project-local directives use a reserved namespace such as `local`, or
  should any non-reserved namespace be allowed?
- Should unknown extension directives ever be preserved without diagnostics?
- Should extension directive schemas be loaded from package manifests,
  project config, or both?
- Which directive effects are allowed before provider capabilities exist?
- Should directive schemas be emitted in compiler metadata for host tools?
- Should system directive short form `@.` be allowed everywhere or only for
  known DSQL directives?
