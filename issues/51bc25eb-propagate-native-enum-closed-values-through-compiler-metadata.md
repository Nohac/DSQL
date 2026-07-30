# Propagate native enum closed values through compiler metadata

**ID:** 51bc25eb | **Status:** Done | **Created:** 2026-07-30T23:17:16+02:00

Native PostgreSQL enum facts now survive introspection, but compiler checks and
generated contracts still treat them as open text-cast scalars. The existing
`enum_values` metadata is only a flat list for synthetic operator and direction
choices; it cannot carry catalog identity, documentation, or result contracts.

Replace that list with one required structured closed-value contract used by
synthetic choices and catalog enums. Infer native values through variables,
reject unknown literal and default variants, validate policy literals, and emit
the ordered rich contract for inputs, trusted context, dynamic fields, and
results. The schema-qualified TextCast provider remains the single nominal
identity; domains inherit values but retain their own identity.

This compiler/metadata slice intentionally does not enforce caller-supplied
values at runtime and does not add editor completion or hover. Generated types
become closed in this step, while runtime enforcement and editor support remain
separate follow-up commits before native enums are declared publicly supported.

Acceptance criteria:

- build manifest version 7 requires `closed_values` and rejects old artifacts;
- native enum comparisons, membership literals, policy literals, and defaults
  reject unknown variants;
- inferred scalar and collection inputs preserve provider identity and ordered
  variants, including domains;
- generated input, context, dynamic, scalar result, and array result metadata
  share the rich catalog-derived contract;
- TypeScript renders literal unions and bypasses open scalar mappings at every
  input/result/dynamic/parser/serializer site; and
- existing synthetic closed choices retain their current semantics.

## Resolution

Manifest version 7 replaces flat synthetic enum lists with one required rich
closed-value set on inputs, bounded dynamic fields, and result values. Native
enum literals, defaults, inferred bindings, policy expressions, metadata, and
TypeScript literal unions now share the catalog's ordered values and preserve
provider identity. Closed values bypass project scalar codecs.

Caller-supplied runtime validation and enum-aware editor services remain the
explicit next slices; native enums stay documented as implementation in
progress until both are complete.
