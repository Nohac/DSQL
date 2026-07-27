# Map logical types to host language types in generators

**ID:** 920e30bf | **Status:** Done | **Created:** 2026-07-26T11:48:49+02:00

The TypeScript renderer maps logical types with a fixed switch and falls back to
`unknown`. A project that stores dates, wants `Temporal.PlainDate` instead of
`string`, or has a custom scalar has no way to say so, even though renderers are
project-owned editable code by design (docs/spec/typescript-distribution.md).

This is the representation half of type support and is deliberately separate from
the semantic half. A host mapping states how a value is spelled and parsed in the
target language; it must not be able to claim a type supports an operator the
database does not.

Accept a scalar mapping in the renderer API, keyed by the nominal logical type
name carried in metadata. Generated source names every dependency explicitly:

```ts
scalars: {
  Date: {
    type: { from: "./date", name: "Date" },
    parse: { from: "./date", name: "parseDate" },
    serialize: { from: "./date", name: "serializeDate" },
  },
}
```

An unmapped logical type derives its TypeScript representation from the
compiler-declared wire encoding. The renderer must not recover the old logical
name switch.

Sequenced after fe7d14cf, which supplies the nominal names.

Acceptance criteria:

- The renderer accepts a scalar map and applies it to result fields, input
  fields, and dynamic input operands alike.
- Optional parse and serialize hooks run at the runtime boundary in both
  directions, so a mapped field never accepts one representation and returns
  another.
- A mapping naming an unknown logical type is an error, not a silent no-op.
- Built-in scalars remain overridable; unmapped input and result types derive
  direction-aware defaults from their wire encodings.
- Renderer tests cover a mapped custom scalar and an unmapped one in the same
  operation.

Implemented as strict project-renderer mappings with named type and paired
codec imports, separate generated host/wire contracts, one-time public and
trusted-context serialization, recursive result parsing, raw TanStack cache
storage, and global multi-target validation. Superseded runtime object shapes
are rejected rather than supported through compatibility fallbacks.
