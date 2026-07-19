# Policy-only hot reload leaves generated modules stale

**ID:** 16ef52ff | **Status:** Done | **Created:** 2026-07-19T11:24:03+02:00

Policy-only edits successfully recompiled and rendered artifacts but could
leave Vite serving the previous generated TypeScript module. Renderer-owned
roots are deliberately excluded from filesystem watching, so rewriting a
stable generated path did not invalidate Vite's client or SSR module graph.

The Vite integration now reports every successfully rendered output directly
to the module graph and reports files removed by a render. This preserves
watcher loop prevention while making policy-only changes visible immediately.
A core SQL regression separately proves that changing a structural match set
already replans tables selected through fragments; no fragment ownership change
was needed.
