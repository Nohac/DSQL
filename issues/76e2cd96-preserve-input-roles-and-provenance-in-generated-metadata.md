# Preserve input roles and provenance in generated metadata

**ID:** 76e2cd96 | **Status:** Open | **Created:** 2026-07-22T18:52:43+02:00

Generated `InputField` metadata drops the compiler's semantic variable role and
the originating definition/span for contained or lifted fragment inputs. This
prevents generators and editor tooling from explaining why an input exists and
from reliably distinguishing pagination, predicate, ordering, and other input
surfaces without reconstructing compiler internals.

Carry the resolved role and optional definition provenance from the effective
binding contract into generated metadata.

Acceptance criteria:

- `InputField` exposes a stable semantic role.
- Definition-derived inputs optionally expose their originating definition and
  source span.
- Containment, root lifting, leaf remapping, and namespaces preserve provenance.
- Metadata schema/mirror, generation, CLI, and TypeScript snapshots are updated.
