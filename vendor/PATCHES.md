# Vendor patch ledger

Every local change to a crate under `vendor/` is recorded here. If a change is
not listed here, it does not exist. See CONTRIBUTING.md for the rules.

## lelwel

- Upstream: <https://github.com/0x2a-42/lelwel>
- Vendored at: `fbd99ce71960489c070620015dd4f8227448262c` (main, post-v0.10.4
  "Fix clippy warnings")
- Method: `git subtree add --squash`

### Local changes

1. **Record expected tokens at parse errors** (carried over from the dsql-poc
   era local checkout).
   - What: generated parsers collect structured `ExpectedToken { token, span }`
     records whenever error recovery reports an expectation, exposed via
     `Cst::expected_tokens()`.
   - Where: `src/skeleton/generated.rs` (skeleton: `ExpectedToken` type,
     `expected_tokens` storage on `Cst`, `Parser::record_expected_tokens`,
     recording in the `expect!`/`try_expect!` macros and end-of-input check),
     `src/backend/rust.rs` (codegen: emit `record_expected_tokens(&[...])`
     with the predict/follow set before every emitted error path),
     `src/frontend/generated.rs` (lelwel's self-hosted parser regenerated with
     the patched generator), `tests/expected_tokens.rs` (new test).
   - Why: drives dsql error recovery and editor completions — knowing *which*
     tokens were viable at the error position, with spans, without parsing
     error-message strings.

## logos

- Upstream: <https://github.com/maciejhirsz/logos>
- Vendored at: tag `v0.16.1` (`8c77ac0`, squashed as `f4846c2`)
- Method: `git subtree add --squash`

### Local changes

None yet.
