//! Extraction of dsql documents embedded in host-language sources.
//!
//! A regex with a named `content` capture finds each region; the capture's
//! text becomes a document of its own, and its byte offset in the host
//! file rides along so diagnostics and source maps can point back into
//! the host source.

use regex::Regex;

use super::config::{Project, ProjectError, Result};

/// One extracted region: the document text and where its first byte sits
/// in the host file.
pub struct EmbeddedRegion {
    pub text: String,
    pub offset: usize,
}

/// The default TypeScript pattern: `dsql`-tagged template literals, with
/// or without a wrapping call.
pub fn default_typescript_pattern() -> &'static str {
    r"dsql(?:\s*\(\s*)?`(?P<content>[\s\S]*?)`(?:\s*\))?"
}

/// The compiled TypeScript embedding of `project`: the configured pattern
/// or the default, validated to define a `content` capture.
pub fn typescript_embedding(project: &Project) -> Result<Regex> {
    let pattern: &str = project
        .config
        .embedding
        .typescript
        .pattern
        .as_deref()
        .unwrap_or(default_typescript_pattern());
    let regex = Regex::new(pattern).map_err(|error| ProjectError::InvalidEmbeddingPattern {
        language: "typescript".to_string(),
        message: error.to_string(),
    })?;
    if regex.capture_names().all(|name| name != Some("content")) {
        return Err(ProjectError::InvalidEmbeddingPattern {
            language: "typescript".to_string(),
            message: "pattern must define a named `content` capture".to_string(),
        });
    }
    Ok(regex)
}

/// Every embedded region of `source`, in file order.
pub fn extract_regions(embedding: &Regex, source: &str) -> Vec<EmbeddedRegion> {
    embedding
        .captures_iter(source)
        .filter_map(|captures| captures.name("content"))
        .map(|content| EmbeddedRegion {
            text: content.as_str().to_string(),
            offset: content.start(),
        })
        .collect()
}
