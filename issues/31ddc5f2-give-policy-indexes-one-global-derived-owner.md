# Give policy indexes one global derived owner

**ID:** 31ddc5f2 | **Status:** Open | **Created:** 2026-08-05T01:31:36+02:00

`index_policies` and `index_policy_bodies` each run once per `ParsedFile` and
have every invocation spawn the same singleton entity. Removing any driver can
therefore reap the shared policy index while surviving file invocations remain
memoized. The trusted-context regression exposed the same ownership defect in
`DefIndex` and `ContextIndex`.

There is also a concrete partial-view reproduction in
`policies::filter_visibility_follows_resolution_scope_imports`: sequentially
inserting two provider files and then their consumer loses the provider-side
ambiguity diagnostic if `index_policies` drops its unrelated `DefIndex` input.
That input currently acts as a post-lowering invalidation bridge rather than a
semantic dependency.

Replace both ambient aggregations with relationship-owned policy facts and one
stable global derived owner. The existing visibility test must stay
byte-identical when the `DefIndex` bridge is removed. Add multi-file removal
and batched source-replacement tests that assert policy diagnostics and plans
stay present.
