# Split trusted-context semantic and navigation invalidation

**ID:** 7ed096cc | **Status:** Done | **Created:** 2026-08-04T23:56:13+02:00

`ContextIndex` hashes declaration name and type spans so cross-file diagnostics
and goto-definition targets move correctly after whitespace edits. That also
causes a whitespace-only edit in a declaring file to rerun variable inference,
policy compilation, and planning for every definition that joins the index.

Split the index into a semantic contract fingerprint and a navigation-only
index. Compiler consumers should depend only on scope, name, resolved type, and
validity; diagnostics and editor navigation should additionally depend on file,
entity, and span payloads. Preserve cross-file target freshness while making
span-only edits semantically fingerprint-neutral.

Variable inference now consumes context-use resolutions through each semantic
group rather than joining `ContextIndex` globally. The resolution component
still hashes declaration navigation spans together with its semantic contract,
so the fact split above remains required.

Declaration semantics and navigation are separate projections. Context uses
and policies bind only the semantic projection through exact name/site
relationships; goto-definition consumes navigation separately. `ContextIndex`
is removed, and a work-shape test proves declaration movement wakes neither
policy compilation nor query planning.
