use dsql_core::TextRange;
use regex::Regex;

pub type Result<T> = std::result::Result<T, EmbeddingError>;

#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("invalid regex embedding pattern: {0}")]
    InvalidRegex(#[from] regex::Error),
    #[error("regex embedding pattern must define a named `content` capture")]
    MissingContentCapture,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedRegion {
    pub ordinal: u32,
    pub content_range: TextRange,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedRegionRange {
    pub ordinal: u32,
    pub content_range: TextRange,
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

    pub fn extract_ranges(&self, source: &str) -> Result<Vec<EmbeddedRegionRange>> {
        extract_regex_ranges(&self.pattern, source)
    }
}

pub fn extract_regex(pattern: &str, source: &str) -> Result<Vec<EmbeddedRegion>> {
    let regex = Regex::new(pattern)?;
    if regex.capture_names().all(|name| name != Some("content")) {
        return Err(EmbeddingError::MissingContentCapture);
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

pub fn extract_regex_ranges(pattern: &str, source: &str) -> Result<Vec<EmbeddedRegionRange>> {
    let regex = Regex::new(pattern)?;
    if regex.capture_names().all(|name| name != Some("content")) {
        return Err(EmbeddingError::MissingContentCapture);
    }

    let mut regions = Vec::new();
    for captures in regex.captures_iter(source) {
        let Some(content) = captures.name("content") else {
            continue;
        };
        regions.push(EmbeddedRegionRange {
            ordinal: regions.len() as u32,
            content_range: TextRange::new(content.start(), content.end()),
        });
    }
    Ok(regions)
}

pub fn default_typescript_regex_pattern() -> String {
    r#"dsql(?:\s*\(\s*)?`(?P<content>[\s\S]*?)`(?:\s*\))?"#.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_regex_returns_typed_error() {
        let error = extract_regex("(?P<content>", "").unwrap_err();

        assert!(matches!(error, EmbeddingError::InvalidRegex(_)));
    }

    #[test]
    fn missing_content_capture_returns_typed_error() {
        let error = extract_regex("(?P<query>.*)", "").unwrap_err();

        assert!(matches!(error, EmbeddingError::MissingContentCapture));
    }

    #[test]
    fn extract_ranges_returns_content_offsets_without_text() {
        let source = "const query = dsql`query Users { users { id } }`;";
        let regions = extract_regex_ranges(&default_typescript_regex_pattern(), source).unwrap();

        assert_eq!(regions.len(), 1);
        assert_eq!(
            &source[regions[0].content_range.as_usize()],
            "query Users { users { id } }"
        );
    }
}
