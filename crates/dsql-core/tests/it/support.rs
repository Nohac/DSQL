//! Shared helpers for the integration harness: fixture loading and
//! snapshot-stable renderers.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use bowl::{Bowl, Entity, Query};
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

/// Renders every diagnostic fact in a settled bowl, sorted for stability.
pub async fn render_diagnostic_facts(bowl: &Bowl) -> String {
    let rows = bowl
        .scoop::<Query<(
            Entity,
            &dsql_core::facts::Severity,
            &dsql_core::facts::Span,
            &dsql_core::facts::Diagnostic,
        )>>()
        .await;
    let mut lines: Vec<String> = rows
        .collect()
        .into_iter()
        .map(|(_, severity, span, diagnostic)| {
            format!(
                "{severity:?}[{}..{}]: {}",
                span.start, span.end, diagnostic.0
            )
        })
        .collect();
    lines.sort();
    lines.join("\n")
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

/// Loads the imdb schema fixtures into a [`dsql_core::catalog::Catalog`].
/// The directory loader lives here until `dsql-project` is ported; it
/// mirrors dsql-poc's `load_metadata_dir`.
pub fn imdb_catalog() -> dsql_core::catalog::Catalog {
    use dsql_core::catalog::{Catalog, SchemaMetadata, TableMetadata, table_metadata_from_yaml};

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it/schema/imdb");
    let mut schemas = Vec::new();
    for entry in std::fs::read_dir(&root).expect("schema fixture dir must exist") {
        let schema_path = entry.expect("schema dir entry").path();
        if !schema_path.is_dir() {
            continue;
        }
        let Some(schema_name) = schema_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let mut tables = Vec::<TableMetadata>::new();
        for table_entry in std::fs::read_dir(&schema_path).expect("table dir") {
            let table_path = table_entry.expect("table dir entry").path();
            if table_path.extension().and_then(|ext| ext.to_str()) != Some("yaml") {
                continue;
            }
            let raw = std::fs::read_to_string(&table_path).expect("table yaml readable");
            let table = match table_metadata_from_yaml(&raw) {
                Ok(table) => table,
                Err(error) => panic!("parse {}: {error}", table_path.display()),
            };
            tables.push(table);
        }
        tables.sort_by(|left, right| left.name.cmp(&right.name));
        schemas.push(SchemaMetadata {
            name: schema_name.to_string(),
            tables,
        });
    }
    schemas.sort_by(|left, right| left.name.cmp(&right.name));

    dsql_core::catalog::DatabaseMetadata { schemas, types: Vec::new() }
        .into_catalog()
        .expect("imdb fixture catalog must build")
        .with_default_schema(Catalog::DEFAULT_SCHEMA)
}
