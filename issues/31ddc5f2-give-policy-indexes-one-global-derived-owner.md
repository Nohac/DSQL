# Give policy indexes one global derived owner

**ID:** 31ddc5f2 | **Status:** Open | **Created:** 2026-08-05T01:31:36+02:00

`index_policies` and `index_policy_bodies` each run once per `ParsedFile` and
have every invocation spawn the same singleton entity. Removing any driver can
therefore reap the shared policy index while surviving file invocations remain
memoized. The trusted-context regression exposed the same ownership defect in
`DefIndex` and `ContextIndex`.

Replace both policy aggregations with one zero-key `TrackedView` invocation,
retain deterministic hash-neutral output, and add multi-file removal and
batched source-replacement tests that assert policy diagnostics and plans stay
present.
