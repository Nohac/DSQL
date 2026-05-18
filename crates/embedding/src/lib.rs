use dsql_core::TextRange;
use miette::{IntoDiagnostic, Result};
use regex::Regex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedRegion {
    pub ordinal: u32,
    pub content_range: TextRange,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegexEmbedding {
    pub pattern: String,
}

impl RegexEmbedding {
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
        }
    }

    pub fn extract(&self, source: &str) -> Result<Vec<EmbeddedRegion>> {
        extract_regex(&self.pattern, source)
    }
}

pub fn extract_regex(pattern: &str, source: &str) -> Result<Vec<EmbeddedRegion>> {
    let regex = Regex::new(pattern).into_diagnostic()?;
    if regex.capture_names().all(|name| name != Some("content")) {
        return Err(miette::miette!(
            "regex embedding pattern must define a named `content` capture"
        ));
    }

    let mut regions = Vec::new();
    for captures in regex.captures_iter(source) {
        let Some(content) = captures.name("content") else {
            continue;
        };
        regions.push(EmbeddedRegion {
            ordinal: regions.len() as u32,
            content_range: TextRange::new(content.start(), content.end()),
            text: content.as_str().to_string(),
        });
    }
    Ok(regions)
}

pub fn default_typescript_regex_pattern() -> String {
    r#"dsql`(?P<content>[\s\S]*?)`"#.to_string()
}
