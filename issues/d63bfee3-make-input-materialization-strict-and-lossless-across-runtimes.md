# Make input materialization strict and lossless across runtimes

**ID:** d63bfee3 | **Status:** Done | **Created:** 2026-07-22T18:52:42+02:00

Rust and TypeScript do not currently implement the same input-materialization
contract. TypeScript recursively converts every object through
`Object.entries`, corrupting values such as `Date`, typed arrays, `Map`, and
`Set`, and loosely converts malformed numeric defaults. Both runtimes can also
treat an explicit null namespace/envelope as omission and plant defaults below
it, although namespace objects are specified as non-null.

Make materialization copy-on-write along only the paths where defaults are
inserted, preserve unrelated host values verbatim, reject non-object
intermediate paths, and enforce the same typed-default validation in Rust and
TypeScript.

Acceptance criteria:

- Materializing one default does not clone or transform unrelated host values.
- Explicit null and scalar intermediate envelopes are errors; defaults are
  never inserted beneath them.
- Integer, finite-float, numeric, collection, and malformed metadata defaults
  have identical accept/reject behavior in both runtimes.
- The executor validates the complete declared input contract, including
  required fields not referenced by the selected SQL variant.
- Trusted `context.*` paths never receive public/default materialization; a
  missing trusted context value is always an error.
- One shared conformance fixture set is exercised by Rust and TypeScript tests.
