//! Embedding configuration: validates the project's TypeScript extraction
//! pattern at the configuration boundary. The extraction itself is a
//! dsql-core system; the loader only inserts host text and the pattern.

use dsql_core::embedding::{compile_embedding_pattern, default_typescript_pattern};

use super::config::{Project, ProjectError, Result};

/// The project's TypeScript embedding pattern — configured or default —
/// validated to compile and define a `content` capture.
pub fn typescript_pattern(project: &Project) -> Result<String> {
    let pattern: &str = project
        .config
        .embedding
        .typescript
        .pattern
        .as_deref()
        .unwrap_or(default_typescript_pattern());
    compile_embedding_pattern(pattern).map_err(|message| {
        ProjectError::InvalidEmbeddingPattern {
            language: "typescript".to_string(),
            message,
        }
    })?;
    Ok(pattern.to_string())
}
