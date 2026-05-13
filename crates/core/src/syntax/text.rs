use facet::Facet;
use ropey::Rope;
use std::borrow::Cow;
use std::ops::Range;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Facet)]
pub struct TextRange {
    pub start: u32,
    pub end: u32,
}

impl TextRange {
    pub fn new(start: usize, end: usize) -> Self {
        Self {
            start: start as u32,
            end: end as u32,
        }
    }

    pub fn as_usize(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

#[derive(Clone, Debug)]
pub struct SourceText {
    pub text: Arc<str>,
}

#[derive(Clone, Debug)]
pub enum SourceSnapshot {
    Text(SourceText),
    Rope(Arc<Rope>),
}

impl SourceSnapshot {
    pub fn from_arc(text: Arc<str>) -> Self {
        Self::Text(SourceText { text })
    }

    pub fn from_string(text: String) -> Self {
        Self::from_arc(Arc::<str>::from(text))
    }

    pub fn from_rope(rope: Rope) -> Self {
        Self::Rope(Arc::new(rope))
    }

    pub fn into_rope(self) -> Rope {
        match self {
            Self::Text(text) => Rope::from_str(text.text.as_ref()),
            Self::Rope(rope) => Arc::try_unwrap(rope).unwrap_or_else(|rope| (*rope).clone()),
        }
    }

    pub fn len_bytes(&self) -> usize {
        match self {
            Self::Text(text) => text.text.len(),
            Self::Rope(rope) => rope.len(),
        }
    }

    pub fn chunks(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        match self {
            Self::Text(text) => Box::new(std::iter::once(text.text.as_ref())),
            Self::Rope(rope) => Box::new(rope.chunks()),
        }
    }

    pub fn text(&self, range: TextRange) -> Cow<'_, str> {
        match self {
            Self::Text(text) => Cow::Borrowed(&text.text[range.as_usize()]),
            Self::Rope(rope) => Cow::Owned(rope.slice(range.as_usize()).to_string()),
        }
    }

    pub fn to_arc_str(&self) -> Arc<str> {
        match self {
            Self::Text(text) => text.text.clone(),
            Self::Rope(rope) => Arc::from(rope.to_string()),
        }
    }
}

impl From<&str> for SourceSnapshot {
    fn from(value: &str) -> Self {
        Self::from_arc(Arc::from(value))
    }
}

impl From<String> for SourceSnapshot {
    fn from(value: String) -> Self {
        Self::from_string(value)
    }
}
