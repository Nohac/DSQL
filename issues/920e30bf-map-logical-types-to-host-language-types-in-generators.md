# Map logical types to host language types in generators

**ID:** 920e30bf | **Status:** Open | **Created:** 2026-07-26T11:48:49+02:00

The TypeScript renderer maps logical types with a fixed switch and falls back to
`unknown`. A project that stores dates, wants `Temporal.PlainDate` instead of
`string`, or has a custom scalar has no way to say so, even though renderers are
project-owned editable code by design (docs/spec/typescript-distribution.md).

This is the representation half of type support and is deliberately separate from
the semantic half. A host mapping states how a value is spelled and parsed in the
target language; it must not be able to claim a type supports an operator the
database does not.

Accept a scalar mapping in the renderer API, keyed by the nominal logical type
name carried in metadata, in the shape GraphQL code generators use:

```ts
scalars: { Date: { type: "Temporal.PlainDate", parse, serialize } }
```

An unmapped logical type keeps today's behaviour and renders as `unknown`.

Sequenced after fe7d14cf, which supplies the nominal names.

Acceptance criteria:

- The renderer accepts a scalar map and applies it to result fields, input
  fields, and dynamic input operands alike.
- Optional parse and serialize hooks run at the runtime boundary in both
  directions, so a mapped field never accepts one representation and returns
  another.
- A mapping naming an unknown logical type is an error, not a silent no-op.
- Built-in scalars remain overridable and default unchanged.
- Renderer tests cover a mapped custom scalar and an unmapped one in the same
  operation.
