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

Directive names are namespace-qualified names after `@`.

```dsql
@dsql.include_if
@table
@table.column
@zod
@zod.email
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

Non-`dsql` directives always use their namespace as the base name. A namespace
may expose a default directive, written as `@namespace`, and member directives,
written as `@namespace.member`:

```dsql
@table(...)
@table.column(...)
@zod(...)
@zod.email(...)
```

`@table` and `@zod` are not bare names. They are default directive invocations
for the `table` and `zod` namespaces. If a namespace does not declare a default
directive, using `@namespace` is a diagnostic.

The only anonymous namespace form is `@.member`, which expands to the reserved
`dsql` namespace:

```dsql
@.include_if   # equivalent to @dsql.include_if
```

Truly bare system directive names are invalid:

```dsql
@include_if   # invalid; use @.include_if or @dsql.include_if
```

Canonical directive identity is modeled as a namespace plus an optional member:

```text
DirectiveName {
  namespace: "table",
  member: None,
}

DirectiveName {
  namespace: "table",
  member: Some("column"),
}
```

Name normalization examples:

```text
@.include_if     -> namespace = "dsql",  member = Some("include_if")
@table           -> namespace = "table", member = None
@table.column    -> namespace = "table", member = Some("column")
@zod             -> namespace = "zod",   member = None
@zod.email       -> namespace = "zod",   member = Some("email")
```

## Directive Invocation Syntax

A directive invocation starts with `@`, followed by a directive name, followed
by an optional argument list.

```text
directive           = "@" directive_name argument_list?
directive_name      = "." directive_member
                    | directive_namespace ("." directive_member)?
directive_namespace = Name
directive_member    = Name
```

```dsql
@zod(schema: "User")
@.include_if(if: $include_posts)
@table.column(label: "Email", hidden: false)
```

Directive arguments are named. Positional directive arguments are not part of
the language.

```dsql
@zod("User") # invalid
```

The argument list uses JSON-shaped DSQL values plus variables and typed
references:

```dsql
@example.directive(
  string_value: "hello",
  number_value: 123,
  boolean_value: true,
  null_value: null,
  variable_value: $enabled,
  column_ref: .email,
  table_ref: public::users,
  object_value: {
    label: "Email",
    input: {
      kind: "email",
      required: true,
    },
  },
  array_value: [
    "one",
    "two",
  ],
)
```

Object keys may be identifiers or quoted strings. Object and array values may
contain nested directive values. Directive object/array syntax is intentionally
JSON-shaped and is not a general expression language: no function calls, spread
syntax, computed keys, or arbitrary operators are part of directive values.
Trailing commas are allowed.

## Directive Placement

Directive placement is source syntax. A directive can appear only where the
grammar allows it, and then the checked directive definition further constrains
whether that placement is valid for the resolved construct.

Proposed source placements:

```dsql
@project.namespace(name: "billing")

query Users @.deprecated(reason: "Use ActiveUsers") {
  users {
    id
  }
}

fragment UserFields on users @tag.public {
  id
  name
}

query UserCards {
  users {
    name @table.column(label: "Name")
    posts @.include_if(if: $include_posts) {
      title
    }
    ...UserFields @.include_if(if: $include_user_fields)
  }
}
```

Document directives apply to the whole source document or bundle member. Query
and fragment directives attach to the declaration header. Selection and fragment
spread directives attach to the specific selection or spread. Future clause
directives may attach to clause lists or individual clauses only if the grammar
adds an unambiguous placement for them.

## Directive Locations

Directive declarations specify the checked locations where the directive is
valid. These are semantic locations, not raw grammar rules.

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
    "namespace": "example",
    "member": "directive",
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
    "namespace": "example",
    "member": "scalar_only",
    "locations": ["scalar_selection"]
  }
}
```

The compiler should report a diagnostic when a directive is used at an
unsupported location.

Syntax placement and semantic location are intentionally separate. The parser
only records that a directive is attached to a document, declaration, selection,
spread, or clause syntax node. Checking resolves that attachment into semantic
locations such as `scalar_selection`, `relation_selection`, or
`fragment_spread`.

## Directive Definitions

All directives have a directive definition. External directives are defined by
directive definition documents. Built-in `dsql` directives may be constructed
programmatically by compiler code, but should use the same schema model for
arguments, validation, completion, and diagnostics.

A definition uses JSON Schema for its argument object and DSQL-specific
extension fields for language metadata.

Directive definition documents should use a single supported JSON Schema
dialect. The initial dialect is JSON Schema 2020-12 unless an implementation
explicitly chooses a narrower dialect. Schema documents should declare the
dialect with `$schema`.

Conceptual shape:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
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
    "namespace": "dsql",
    "member": "include_if",
    "locations": ["field_selection", "fragment_spread"]
  }
}
```

The JSON value validated by the schema is the directive argument object.

For:

```dsql
@zod(schema: "User")
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

## Schema Representation And Tooling

The compiler should treat directive schemas as JSON Schema data, not primarily
as Rust type-derived schemas. A directive definition needs three related
representations:

```text
DirectiveDefinition {
  name: DirectiveName,
  kind: BuiltIn | External,
  locations: Vec<DirectiveLocation>,
  repeatable: bool,
  raw_schema: JsonValue,
  schema_ast: DirectiveSchemaAst,
  validator: CompiledJsonSchemaValidator,
}
```

`kind` is determined by the registration path, not by the JSON Schema document.
Built-in `dsql` directives are registered by compiler code. External directive
schemas are registered by project configuration, provider packages, or codegen
integrations and are metadata/codegen-only.

`raw_schema` preserves the authored schema document for standards-compliant
validation, round-tripping, diagnostics, and unsupported keywords.
`schema_ast` is a normalized compiler-owned projection used by checking,
completion, hover, semantic tokens, and generation metadata. `validator` is an
opaque compiled JSON Schema validator used only for JSON Schema validation.

This representation should be shared with other schema-backed language features
where possible. In particular, JSON/JSONB column shape overrides need the same
raw schema, normalized schema AST, and validator boundary, but attach it to a
catalog column or JSON path instead of to a directive invocation.

The compiler should not rely on a validator crate as the editor-facing schema
AST. Validator APIs typically optimize for validation and either keep their
compiled schema private or expose details that are not stable enough for DSQL
language semantics.

The normalized directive schema AST should initially cover only the subset DSQL
needs to reason about directive invocations:

```text
DirectiveSchemaAst {
  root: DirectiveValueSchema,
  raw_schema: JsonValue,
}

DirectiveValueSchema {
  value_shape: JsonValueShape,
  description: Option<String>,
  default: Option<JsonValue>,
  enum_values: Vec<JsonValue>,
  dsql_ref: Option<DirectiveReferenceKind>,
  dsql_expression: bool,
  object: Option<DirectiveObjectSchema>,
  array_items: Option<Box<DirectiveValueSchema>>,
  raw_schema: JsonValue,
}

DirectiveObjectSchema {
  properties: Map<String, DirectiveValueSchema>,
  required: Set<String>,
  additional_properties: AdditionalPropertiesPolicy,
}
```

The AST may preserve unsupported or unnormalized schema keywords in `raw_schema`
without making them available for editor intelligence. For example, a schema
using complex `oneOf` branches can still be passed to the JSON Schema validator,
while completion only offers argument names and enum values that the
normalization step can derive unambiguously. Nested object and array schemas are
normalized recursively so completion can offer object keys, enum values,
booleans, and typed references inside nested directive structures.

Programmatic directive definitions should construct this compiler model
directly and be able to emit the equivalent JSON Schema. They do not need to be
derived from Rust structs. A builder-style API is acceptable for built-in system
directives and provider registrations:

```rust
DirectiveDefinition::new(DirectiveName::member("dsql", "include_if"))
    .locations([DirectiveLocation::FieldSelection, DirectiveLocation::FragmentSpread])
    .argument(
        "if",
        DirectiveArgumentSchema::boolean()
            .required()
            .expression(true)
            .description("Controls whether the selection is included"),
    );
```

The `facet-json-schema` crate exposes public `JsonSchema`, `SchemaType`,
`SchemaTypes`, and `AdditionalProperties` types that can be built and traversed
manually, and may be a useful base for this model. The older
`facet-jsonschema` crate currently used in the workspace is a different crate
with a string-emitting API and is not suitable as the directive schema model.

Before implementation, run a focused dependency update/design pass to decide
whether to:

- move to the newer `facet-json-schema` crate and matching Facet version;
- vendor or fork a small JSON Schema data model;
- define a DSQL-owned directive schema model and only convert to/from JSON
  values at the registry boundary.

## Directive Value Model

Directive source values are not passed directly to JSON Schema consumers. The
compiler first parses them as DSQL syntax and normalizes them into a checked
directive value model.

Conceptual shape:

```text
DirectiveValue =
  Null(source_range)
  | Boolean(value, source_range)
  | Number(value, source_range)
  | String(value, source_range)
  | Variable(VariableRef, source_range)
  | TypedReference(ReferenceKind, ResolvedReference, source_range)
  | Array(Vec<DirectiveValue>, source_range)
  | Object(Vec<(Name, DirectiveValue)>, source_range)
```

JSON Schema validates the public JSON-compatible shape of the argument object.
The DSQL-specific extensions then decide whether a source value is allowed to be
an expression, variable, or typed reference, and how that reference resolves.

This gives the compiler one boundary between syntax and metadata:

1. parse directive syntax from source;
2. normalize names and argument values into checked directive values;
3. validate JSON-compatible shape;
4. resolve DSQL-specific references and variables;
5. expose checked directive metadata to generation, editor, and provider stages.

Generators should consume checked directive values, not raw syntax and not
already-stringified JSON, so source ranges and resolved references remain
available for diagnostics and editor features.

## System Directives

System directives are owned by the DSQL language and live in the `dsql`
namespace. They may be written with `@dsql.` or the short `@.` form.

System directives are compiled by DSQL itself. If a system directive changes
checking, planning, SQL, generated metadata, variables, or editor behavior, that
behavior must be implemented in the compiler.

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
    email @zod.email
    name @table.column(label: "Display name")
  }
}
```

Extension directives are metadata/codegen-only in the initial model. They can
validate their argument shape, resolve declared typed references, power editor
support, and preserve checked metadata for generators. They cannot change core
checking, planning, SQL generation, result shape, or runtime semantics through a
schema alone.

```json
{
  "x-dsql-directive": {
    "namespace": "zod",
    "member": "email",
    "locations": ["scalar_selection"]
  }
}
```

Generator entrypoints decide which external directive namespaces they consume.
The compiler should validate and preserve checked external directives, but it
should not interpret non-`dsql` directive metadata inside core semantic stages.

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
relation_selector
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
@example.table(target: public::users)
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
selection
argument
```

`selection` means the reference resolves against the selected item context for a
directive-specific nested renderer. For example, a relation-level table column
renderer can use `selection` so nested `label: .title` references resolve
against the related item rather than the parent table.

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
@example.prefetch(relation: public::posts->user_id)
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

### `relation_selector`

A relation edge selector such as the part after `->`.

```json
{
  "type": "string",
  "x-dsql-ref": "relation_selector",
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
users @table(default_sort: .email) {
  id
  email
}
```

The semantic distinction is that the referenced field must exist in the selected
result shape, not merely in the catalog.

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
  users @table(title: output.name) {
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

Some directive use cases need to reference a query together with bindings for
that query's inferred public inputs. For example, a cache directive may point to
a separate cache-key query and say how the current query's inferred params or
input should be forwarded:

```dsql
query UserCard @cache(
  invalidated_by: [
    {
      query: UserCardCacheKey(
        params: { id: params.user_id },
      ),
      keys: [.updated_at],
    },
  ],
) {
  users(where .id == $$user_id) {
    id
    name
  }
}

query UserCardCacheKey {
  users(where .id == $$id) {
    id
    updated_at
  }
}
```

This should not become a directive-specific variable system. The binding syntax
should reuse the same checked model as fragment variable lifting: a target
definition infers its own `params` and `input`, and the caller may bind those
target paths from input paths available at the call site. Context is not bound
here because `$:name` context values are global host inputs and are supplied by
the runtime in the usual way.

The binding semantics are defined by the bound definition input model in the
variables spec. The exact bound-query schema extension is still deferred.
Conceptually, the directive schema needs to say that a `query` reference may
carry a call payload and that sibling refs such as `keys` resolve against the
query selected by that bound reference:

```json
{
  "query": {
    "type": "string",
    "x-dsql-ref": "query",
    "x-dsql-call": {
      "params": true,
      "input": true
    },
    "x-dsql-bind-ref": "cache_query"
  },
  "keys": {
    "type": "array",
    "items": {
      "type": "string",
      "x-dsql-ref": "output_path",
      "x-dsql-query": { "ref": "cache_query" }
    }
  }
}
```

## Reference Syntax

Directive arguments may use ordinary literals or typed references.

Examples:

```dsql
@example.table(target: public::users)
@example.column(target: .email)
@example.relation(target: posts)
@example.fragment(target: UserFields)
```

Whether an unquoted name is a string or a typed reference depends on the
directive schema. If the argument schema has `x-dsql-ref`, the compiler resolves
the value as a typed reference. Otherwise a bare name is invalid and the user
should use a string literal.

```dsql
@zod(schema: "EmailAddress")          # string
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
namespace = "table"
schema = "dsql/directives/table.schema.json"
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
invalidate directive checking, editor completion, and checked metadata consumed
by generation.

Registry precedence:

1. built-in `dsql` system directives;
2. explicitly configured project-local directives;
3. provider package directives;
4. generator or adapter integration directives.

The `dsql` namespace is reserved and cannot be replaced by project or provider
schemas. For extension namespaces, duplicate definitions for the same fully
qualified directive name are diagnostics unless the project config explicitly
chooses one provider as the owner. Silent last-writer-wins merging is invalid
because directive schemas affect validation, editor behavior, and generated
metadata.

## Validation Rules

The compiler validates directives in this order:

1. Parse directive syntax.
2. Normalize directive names, including `@.` to `@dsql.`.
3. Resolve the directive definition from the registry.
4. Normalize the directive schema into the compiler-owned directive schema AST.
5. Validate the directive location.
6. Validate argument names, required arguments, and JSON Schema shape using the
   compiled validator and source-range mapping.
7. Resolve typed references using the directive schema AST and current semantic
   context.
8. Validate variable usage and infer directive variables when allowed.
9. Produce checked directive metadata for compiler-owned built-ins and codegen
   entrypoints.

Diagnostics should include:

- unknown directive;
- unknown directive namespace;
- directive not allowed at location;
- missing required argument;
- unknown argument;
- invalid argument type;
- invalid typed reference;
- unresolved typed reference;
- duplicate directive when a directive is declared non-repeatable.

## Repeatability

Directives are non-repeatable by default at the same location.

The directive definition may opt into repeatability:

```json
{
  "x-dsql-directive": {
    "namespace": "tag",
    "member": "label",
    "locations": ["field_selection"],
    "repeatable": true
  }
}
```

If a non-repeatable directive appears twice on the same construct, the compiler
reports a diagnostic.

Repeatable directives preserve source order in checked metadata. Consumers must
not sort repeatable directives unless the directive definition declares that
order is irrelevant.

Order-sensitive repeatable directives should say so explicitly:

```json
{
  "x-dsql-directive": {
    "namespace": "pipeline",
    "member": "step",
    "locations": ["query"],
    "repeatable": true,
    "orderSensitive": true
  }
}
```

If a directive is repeatable and order-sensitive, generation and provider stages
must consume the checked directives in source order.

## Metadata Preservation

Metadata directives should be preserved in checked compiler outputs in a
structured form.

Conceptual shape:

```text
CheckedDirective {
  name: DirectiveName,
  location: DirectiveLocation,
  arguments: Map<String, CheckedDirectiveValue>,
  source_range: TextRange,
}
```

Generation should receive checked directives, not raw AST directives. This
prevents generators from revalidating directive names, argument types, and
catalog references.

External checked directives should not affect SQL, result shape, or core
semantic checking. Generators may interpret external checked metadata at their
entrypoints.

## Editor Support

Directive definitions should power editor behavior:

- after `@`, complete available namespaces for the current location, including
  namespaces with default directives and namespaces with members;
- after `@.`, complete DSQL system directives;
- after `@namespace.`, complete member directives in that namespace;
- inside argument lists, complete valid argument names;
- inside nested objects, complete valid object property names from the recursive
  schema AST;
- for `enum` arguments, complete allowed enum values, inserting quoted strings
  for string-valued enums;
- for boolean arguments, complete `true` and `false`;
- for typed reference arguments, complete catalog tables, columns, relations,
  fragments, output paths, or selected fields according to `x-dsql-ref`;
- hover on directive names should show schema `description`;
- hover on directive arguments should show schema property `description`;
- semantic tokens should classify directive names and typed references.

Completion should use the normalized directive schema AST, not an opaque JSON
Schema validator. It must use the selected resolution environment and current
catalog snapshot.

## Formatting

The formatter may normalize directive spacing:

```dsql
field @table.column(label: "Name")
```

Long directive argument lists may be split:

```dsql
field @table.column(
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
    email @zod.email
    age @zod(schema: "AdultAge")
  }
}
```

### Catalog-Aware Extension Directive

```dsql
query Users {
  users @table(default_sort: .email) {
    id
    email @table.column(label: "Email")
  }
}
```

The `default_sort` argument can declare `x-dsql-ref: "column"` with
`x-dsql-table: "current"`, so completion and validation use the `users` table.

### Nested Directive Metadata

Nested object and array values are useful for codegen metadata that naturally
belongs to the selected field or relation.

```dsql
query Users {
  users @table(
    empty_state: {
      title: "No users",
      action: {
        label: "Create user",
        route: "/users/new",
      },
    },
  ) {
    id
    email @table.column(
      label: "Email",
      sortable: true,
      input: {
        kind: "email",
        required: true,
        placeholder: "user@example.com",
      },
    )
    posts @table.column(
      label: "Posts",
      view: {
        kind: "chips",
        label: .title,
        value: .id,
        max: 4,
      },
    ) {
      id
      title
    }
  }
}
```

In this example, `@table` is the default directive for the `table` namespace and
`@table.column` is a member directive. A default directive uses `member: null`:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://example.com/dsql/directives/table.schema.json",
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "default_sort": {
      "type": "string",
      "x-dsql-ref": "column",
      "x-dsql-table": "current"
    },
    "empty_state": {
      "type": "object",
      "additionalProperties": false,
      "properties": {
        "title": { "type": "string" },
        "action": {
          "type": "object",
          "additionalProperties": false,
          "required": ["label", "route"],
          "properties": {
            "label": { "type": "string" },
            "route": { "type": "string" }
          }
        }
      }
    }
  },
  "x-dsql-directive": {
    "namespace": "table",
    "member": null,
    "locations": ["relation_selection"]
  }
}
```

The relation-level `@table.column` attaches to the relation selection itself,
because that relation occupies a column in the parent table. Nested field
references such as `label: .title` resolve against the relation item context
declared by the directive schema.

The corresponding `@table.column` schema can attach typed references at nested
schema nodes:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://example.com/dsql/directives/table.column.schema.json",
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "label": { "type": "string" },
    "sortable": { "type": "boolean", "default": false },
    "input": {
      "type": "object",
      "additionalProperties": false,
      "properties": {
        "kind": {
          "type": "string",
          "enum": ["text", "email", "number", "date"]
        },
        "required": { "type": "boolean", "default": false },
        "placeholder": { "type": "string" }
      }
    },
    "view": {
      "type": "object",
      "additionalProperties": false,
      "required": ["kind"],
      "properties": {
        "kind": {
          "type": "string",
          "enum": ["chips", "count", "summary"]
        },
        "label": {
          "type": "string",
          "x-dsql-ref": "field",
          "x-dsql-table": "selection"
        },
        "value": {
          "type": "string",
          "x-dsql-ref": "field",
          "x-dsql-table": "selection"
        },
        "max": { "type": "integer", "minimum": 1 }
      }
    }
  },
  "x-dsql-directive": {
    "namespace": "table",
    "member": "column",
    "locations": ["scalar_selection", "relation_selection"]
  }
}
```

## Open Questions

- Should output-path and selected-field references have dedicated syntax?
- What exact JSON Schema extension should identify bound query references used
  by directives such as `@cache`?
- Should project-local directives use a reserved namespace such as `local`, or
  should any non-reserved namespace be allowed?
- Should unknown extension directives ever be preserved without diagnostics?
- Should extension directive schemas be loaded from package manifests,
  project config, or both?
- Should directive schemas be emitted in compiler metadata for host tools?
- Should system directive short form `@.` be allowed everywhere or only for
  known DSQL directives?
