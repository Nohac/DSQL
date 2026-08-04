# Split trusted-context semantic and navigation invalidation

**ID:** 7ed096cc | **Status:** Open | **Created:** 2026-08-04T23:56:13+02:00

`ContextIndex` hashes declaration name and type spans so cross-file diagnostics
and goto-definition targets move correctly after whitespace edits. That also
causes a whitespace-only edit in a declaring file to rerun variable inference,
policy compilation, and planning for every definition that joins the index.

Split the index into a semantic contract fingerprint and a navigation-only
index. Compiler consumers should depend only on scope, name, resolved type, and
validity; diagnostics and editor navigation should additionally depend on file,
entity, and span payloads. Preserve cross-file target freshness while making
span-only edits semantically fingerprint-neutral.
