# Add LSP semantic token wire coverage

**ID:** 5fc6bba7 | **Status:** Done | **Created:** 2026-07-17T01:46:36+02:00

The LSP advertises and implements `textDocument/semanticTokens/full`, while
current coverage stops at core token classification and host projection. Add
an in-process protocol test that verifies the advertised legend, absolute to
delta encoding, UTF-16 coordinates, and embedded-region projection at the
adapter boundary.

Keep the test independent of production position conversion so it remains a
wire-format oracle.
