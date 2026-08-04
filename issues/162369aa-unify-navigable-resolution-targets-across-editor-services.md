# Unify navigable resolution targets across editor services

**ID:** 162369aa | **Status:** Open | **Created:** 2026-08-04T22:15:42+02:00

Editor services project catalog targets independently from the same semantic
resolution facts. The projections have drifted: predicate aggregate relations
and operands in `ResolvedClause::aggregates` receive hover and semantic tokens,
but goto-definition only visits ordinary paths and order items. For example,
the relation in `(.posts | count)` can hover while goto-definition returns
nothing. Existence relation and table sources in `ResolvedClause::existences`
are not handled consistently by any of the three services.

Expose one shared navigable target representation from semantic resolution and
make hover, goto-definition, and semantic tokens consume it. Do not add another
syntax walker or service-specific resolver. Service presentation may still
differ, but whether a span resolves and which catalog object it targets must
come from one fact.

Acceptance criteria:

- Shared resolved targets cover ordinary relation steps, terminal columns,
  predicate aggregate relation steps and operands, existence relation and
  table sources, root/parent anchors, and order items.
- Resolution facts gain source spans for navigable targets such as anchors that
  do not carry one today; this remains semantic resolution data, not a new
  service resolver.
- Goto-definition works for the aggregate relation regression and lands on the
  same catalog relation represented by hover.
- Hover, goto-definition, and semantic tokens consume the shared target model
  without re-resolving source text or maintaining separate target walkers.
- Unsupported targets are represented explicitly rather than silently omitted
  by one service.
- Integration snapshots exercise every target class in plain and embedded
  documents, including host-coordinate projection.
