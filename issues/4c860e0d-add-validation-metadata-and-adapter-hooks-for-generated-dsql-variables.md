# Add validation metadata and adapter hooks for generated DSQL variables

**ID:** 4c860e0d | **Status:** Open | **Created:** 2026-06-13T14:23:24+02:00

## Summary

Add stable validation metadata and adapter hooks so users can plug in runtime
validation for generated DSQL variables without making core DSQL depend on a
validation library.

## Context

Generated TypeScript currently provides static types for `params` and `input`,
but framework wrappers still need runtime validation in many applications. Users
may want to hand-write validators with Zod, ArkType, Valibot, or custom code.
Other users may want a validation generator that interprets future directive
metadata such as `@zod.min(1)` on DSQL variables.

Core DSQL should facilitate both paths while staying validation-library
agnostic.

## User-Written Validator Example

```ts
import { z } from "zod";
import type { DsqlVariables } from "@dsql/typescript/runtime";
import { MovieLookupOperation } from "@/generated/dsql/frontend/queries";

export const MovieLookupVariablesSchema = z.object({
  params: z.object({}),
  input: z.object({
    movie_info: z.object({
      clause: z.object({
        where: z.object({
          id: z.number().int().positive(),
        }),
        limit: z.object({
          limit: z.number().int().min(1).max(50),
        }),
      }),
    }),
  }),
}) satisfies z.ZodType<DsqlVariables<typeof MovieLookupOperation>>;
```

Framework adapter hookup:

```ts
await renderTanStackStart(artifacts, dsql, {
  validatorFor(operation) {
    if (operation.name === "MovieLookup") {
      return {
        import: {
          name: "MovieLookupVariablesSchema",
          from: "@/validation/movie-lookup",
        },
        expression: "MovieLookupVariablesSchema.parse",
      };
    }

    return "identity";
  },
});
```

## Directive-Based Validation Example

DSQL could allow metadata annotations on variables. This is a future syntax
sketch, not syntax supported by the current grammar:

```dsql
query MovieLookup {
  movie_info(
    where .id == $ @zod.int @zod.positive
    limit $limit @zod.int @zod.min(1) @zod.max(50)
  ) {
    id
    info
  }
}
```

Core should parse and preserve directive metadata, but not interpret `zod.*`
itself. A validation generator can consume the metadata and emit Zod schemas.
Another plugin could ignore those directives or interpret a different directive
namespace.

Current DSQL grammar only supports bare directives like `@name` on fragment
spreads and field selections. Supporting the example above requires explicit
grammar work for at least:

- directive attachment on variables or expressions
- dotted directive names such as `zod.min`
- directive arguments such as `(1)`
- lowering and metadata preservation for directive source ranges

Directive grammar support is intentionally out of scope for the first
validation baseline. The first milestone should focus on stable generated
variable types, variable metadata, and adapter hooks for user-authored
validators. Directive-driven validation can be added later once the baseline is
solid.

Potential metadata shape:

```ts
{
  path: "input.movie_info.clause.where.id",
  dataType: "number",
  required: true,
  nullable: false,
  directives: [
    { name: "zod.int", args: [] },
    { name: "zod.min", args: ["1"] },
    { name: "zod.max", args: ["50"] }
  ],
  source: { file: "queries/movie.dsql", range: { start: 42, end: 48 } }
}
```

## Design Constraints

- Core runtime accepts already-validated variables and should only report
  materialization errors such as missing variant values or missing parameter
  paths.
- Core TypeScript generation should expose stable input metadata, generated
  variable types, and source ranges.
- Adapter generators should accept validator hooks or generated validator maps.
- Validation generators should be optional and library-specific.
- Directives should be pass-through metadata with names, args, and source
  ranges; core should not hardcode Zod behavior.
- Validator examples and generated metadata must use the same variable path
  semantics as the compiler. Anonymous values, named values, anonymous
  operator/value pairs, top-level params, and fragment envelopes all produce
  different paths and need focused tests.

## Done When

- Generated TypeScript exposes `DsqlVariables<Operation>` and enough metadata to
  build validators for `params` and `input`.
- Framework adapters can use an identity validator by default or a user-provided
  validator expression per operation.
- Custom validators can be type-checked against generated DSQL variable types.
- Optional directive metadata is preserved on variable/input metadata without
  coupling core to a validation library.
- Grammar support for variable/expression directives is explicitly deferred
  until after the baseline validation hook and metadata model exists.
- A Zod proof-of-concept generator or test fixture demonstrates directive-based
  validation in a later milestone.
- Tests cover user-authored validator imports, identity fallback, and generated
  validator hookup in at least one adapter.
