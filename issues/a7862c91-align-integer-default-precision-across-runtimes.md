# Align integer default precision across runtimes

**ID:** a7862c91 | **Status:** Open | **Created:** 2026-07-22T22:12:52+02:00

Rust materializes integer defaults across the full signed 64-bit range, while
the TypeScript runtime validates that same range and then converts it to a
JavaScript `number`. Values outside the safe-integer range are accepted by both
runtimes but lose precision in TypeScript, so omitted defaults can differ from
Rust execution and from an explicitly supplied host value.

Choose and specify one wire contract: restrict integer defaults to safe host
numbers in the compiler and both runtimes, or introduce an exact integer
representation that also works in generated types and cache keys. Add shared
conformance cases at both boundaries and update the operation-execution spec.
