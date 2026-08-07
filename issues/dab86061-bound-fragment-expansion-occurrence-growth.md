# Bound fragment expansion occurrence growth

**ID:** dab86061 | **Status:** Open | **Created:** 2026-08-06T20:53:49+02:00

Fragment expansion materializes one occurrence per simple spread path so all
semantic consumers can share tracked, path-sensitive bodies. A branching
fragment graph can therefore grow exponentially even though name-based cycle
cutoff guarantees convergence. The depth-eight doubling fixture currently
produces 511 occurrences for its query root and 1,515 across all roots.
Every lowered spread additionally owns one dedicated semantic-site group so
candidate and cycle inverses never land on syntax entities; the same fixture
has 17 such groups (16 fragment spreads and one query spread). Each group
drives one resolved-candidate computation and one cycle-check invocation.
Candidate binding is exact by fragment name, so provider edits scale with the
number of same-name spread sites rather than every spread in the project.
Each occurrence body also clones and hashes its context-use resolutions; all
1,515 bodies in the depth fixture currently carry an empty context vector.

Before accepting untrusted project input, define and enforce a deterministic
per-root occurrence budget. Exceeding it should emit one stable diagnostic at
the spread frontier and stop extending that root without affecting unrelated
roots. Keep the existing repeated-spread, diamond, cycle, and cold/incremental
equivalence coverage when adding the limit.
