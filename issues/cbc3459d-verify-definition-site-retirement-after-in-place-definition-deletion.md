# Verify definition-site retirement after in-place definition deletion

**ID:** cbc3459d | **Status:** Open | **Created:** 2026-08-07T23:28:59+02:00

`project_definition_sites` emits producer-owned `DefinitionSiteRoot` entities
from an unbound definition query and gives them no `DerivedFrom` anchor. Verify
with an editor-style `SourceText::set_text` test that deleting a definition
while keeping its file entity retires the site, its context, and all visible
candidate relationships. Whole-file despawn tests do not exercise this path.

If the site lingers, replace the lifetime edge with a narrow tracked projection
or make retirement pair-bound. Do not anchor candidates to the mutable syntax
definition or to a relationship target; follow the ownership invariant in
`docs/architecture/compiler.md`.
