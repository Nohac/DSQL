# Remove unmaintained paste dependency from Porridge

**ID:** 58699323 | **Status:** Open | **Created:** 2026-11-12T13:19:18+01:00

Cargo audit reports [RUSTSEC-2024-0436](https://rustsec.org/advisories/RUSTSEC-2024-0436.html):
`paste` 1.0.15 is unmaintained. It is pulled in by
`bowl -> async_fn_traits 0.1.1 -> paste`; no patched paste version exists.

Replace or remove this dependency upstream in Porridge, then bump the pinned
Porridge revision in a dedicated commit. Keep the audit warning visible until
resolved, and verify workspace tests and cargo audit after updating.
