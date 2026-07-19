# Preserve later embedded expressions when earlier regions move

**ID:** cca4fdca | **Status:** Done | **Created:** 2026-07-19T21:15:36+02:00

Editing an embedded dsql region with a length-changing edit shifts every later
region's host-coordinate spans. Those projection changes currently retire the
later regions' semantic facts even though their source text is unchanged, so
the memoized parse does not rebuild them. The later expressions then report
that they contain no top-level definition until another event heals the host.

Host-coordinate projection data must remain current without participating in
the extracted document's semantic revision. Add a multi-region regression that
changes an earlier expression, verifies later region identity and shifted
coordinates, and keeps every expression resolved without shape diagnostics.

Resolved by keeping host-coordinate offsets and callsite/content spans
untracked. Extraction still refreshes their latest values, while only semantic
document changes can retire facts derived from an extracted region. The
multi-region regression covers a length-changing edit above unchanged queries.
