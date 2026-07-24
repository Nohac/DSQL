# Reuse source content digest for daemon callsite hashes

**ID:** e4429d53 | **Status:** Open | **Created:** 2026-07-24T15:45:01+02:00

Daemon callsite assembly materializes each embedding host and computes a fresh
SHA-256 digest even though [`SourceText`] already fingerprints the same content.
The existing `SourceText::content_hash()` is only a process-local `u64`, while
the build-daemon protocol requires a stable lowercase SHA-256 content hash, so
it cannot be substituted directly.

Store or memoize the protocol-grade digest at the source boundary and expose it
without re-materializing or re-hashing resident text. Keep the engine's compact
content fingerprint and the protocol digest derived from one source update so
they cannot disagree.

Acceptance criteria:

- callsite response assembly does not materialize host text solely to hash it;
- the returned digest remains the exact SHA-256 required by the daemon spec;
- inserts and every edit path update the digest together with the rope content;
- residency eviction retains enough information to answer the protocol; and
- focused source and daemon protocol tests cover unchanged, edited, and
  evicted host content.
