//! Integration test harness for `dsql-core`. One binary, one module per
//! area, mirroring the layout described in CONTRIBUTING.md.

mod parse;
mod settle;

use std::fmt::Write;
use std::path::{Path, PathBuf};

use codespan_reporting::diagnostic::Severity;
use dsql_core::grammar::parser::Diagnostic;

/// Directory holding the shared `.dsql` fixture queries.
pub fn queries_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it/queries")
}

/// Reads a fixture query by its path relative to [`queries_dir`].
pub fn fixture(relative_path: &str) -> String {
    let path = queries_dir().join(relative_path);
    match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => panic!("failed to read fixture {}: {error}", path.display()),
    }
}

/// Renders parse diagnostics into a compact, snapshot-stable form.
pub fn render_diagnostics(source: &str, diagnostics: &[Diagnostic]) -> String {
    let mut rendered = String::new();
    for diagnostic in diagnostics {
        let severity = match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
            Severity::Help => "help",
            Severity::Bug => "bug",
        };
        let span = diagnostic
            .labels
            .first()
            .map(|label| label.range.clone())
            .unwrap_or(0..0);
        let excerpt = source.get(span.clone()).unwrap_or("<out of range>");
        writeln!(
            rendered,
            "{severity}[{}..{}]: {} ({excerpt:?})",
            span.start, span.end, diagnostic.message
        )
        .expect("writing to a String cannot fail");
    }
    rendered
}
