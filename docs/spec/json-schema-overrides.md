# JSON Schema Overrides

Status: consideration.

JSON and JSONB columns often have application-level structure that is not
visible from the database catalog alone. dsql should consider allowing users to
define schema shapes for specific JSON-like columns.

## Use Case

Catalog metadata may know only this:

```text
public.events.payload jsonb
```

Project metadata could refine it:

```text
public.events.payload -> EventPayload schema
```

This would let dsql improve:

- hover information
- completion inside JSON field paths
- type checking for generated code
- runtime validation schema generation
- frontend form/table metadata

## Shared Schema Model

JSON/JSONB overrides should reuse the schema representation being specified for
directives: an authored raw JSON Schema document, a compiler-owned normalized
schema AST for completion/hover/type analysis, and an opaque compiled validator
for standards-compliant validation.

The attachment point differs from directives. A directive schema describes the
argument object accepted by a directive invocation. A JSON/JSONB override schema
describes the value shape owned by a catalog column or nested JSON path. Both
features still need the same handling for object properties, required fields,
additional properties, enum values, defaults, descriptions, composition, and
unsupported keywords.

Keeping the schema model shared avoids separate completion and validation
implementations for directive arguments and JSON field paths. Feature-specific
layers should add only ownership and semantic context:

- directives attach schemas to directive names, locations, and effects;
- JSON/JSONB overrides attach schemas to catalog columns, provider-specific JSON
  operators, and generated result/input paths.

## Open Questions

- Whether overrides live in project config, schema metadata files, or separate
  schema files.
- Whether the schema format should be JSON Schema, TypeScript-derived, or a
  dsql-native shape.
- How nested JSON paths are queried in dsql.
- How provider-specific JSON operators interact with typed schema paths.
- How schema overrides are validated against real data, if at all.
