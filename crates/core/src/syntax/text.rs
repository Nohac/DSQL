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

    pub fn len(self) -> usize {
        self.end.saturating_sub(self.start) as usize
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// Immutable source text snapshot used by compiler stages.
///
/// Source snapshots are always Rope-backed so editor and project source can
/// cross compiler boundaries without first flattening into a contiguous string.
#[derive(Clone, Debug)]
pub struct SourceSnapshot {
    rope: Arc<Rope>,
}

impl SourceSnapshot {
    pub fn from_string(text: String) -> Self {
        Self::from_rope(Rope::from_str(&text))
    }

    pub fn from_rope(rope: Rope) -> Self {
        Self {
            rope: Arc::new(rope),
        }
    }

    pub fn from_arc_rope(rope: Arc<Rope>) -> Self {
        Self { rope }
    }

    pub fn as_rope(&self) -> &Rope {
        &self.rope
    }

    pub fn arc_rope(&self) -> Arc<Rope> {
        self.rope.clone()
    }

    pub fn as_contiguous_str(&self) -> Option<&str> {
        let mut chunks = self.rope.chunks();
        let chunk = chunks.next()?;
        chunks.next().is_none().then_some(chunk)
    }

    /// Returns the full source text as a contiguous string.
    ///
    /// If the underlying Rope is already one contiguous chunk, the returned
    /// string borrows from this snapshot. Otherwise, the rope is flattened into
    /// an owned [`String`].
    pub fn full_text(&self) -> Cow<'_, str> {
        if let Some(text) = self.as_contiguous_str() {
            Cow::Borrowed(text)
        } else {
            Cow::Owned(self.rope.to_string())
        }
    }

    pub fn into_rope(self) -> Rope {
        Arc::try_unwrap(self.rope).unwrap_or_else(|rope| (*rope).clone())
    }

    pub fn len_bytes(&self) -> usize {
        self.rope.len()
    }

    pub fn byte_at(&self, byte: usize) -> Option<u8> {
        if byte >= self.len_bytes() {
            return None;
        }
        self.rope.bytes_at(byte).next()
    }

    pub fn bytes_eq(&self, start: usize, expected: &[u8]) -> bool {
        let Some(end) = start.checked_add(expected.len()) else {
            return false;
        };
        if end > self.len_bytes() {
            return false;
        }
        self.rope
            .bytes_at(start)
            .take(expected.len())
            .eq(expected.iter().copied())
    }

    pub fn chunks(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.rope.chunks())
    }

    pub fn contiguous_text(&self, range: TextRange) -> Option<&str> {
        let range = range.as_usize();
        if range.end > self.len_bytes() {
            return None;
        }
        if range.is_empty() {
            return Some("");
        }
        let (mut chunks, chunk_start) = self.rope.chunks_at(range.start);
        let chunk = chunks.next()?;
        let start = range.start - chunk_start;
        let end = start.checked_add(range.len())?;
        chunk.get(start..end)
    }

    pub fn text(&self, range: TextRange) -> Cow<'_, str> {
        if let Some(text) = self.contiguous_text(range) {
            Cow::Borrowed(text)
        } else {
            Cow::Owned(self.rope.slice(range.as_usize()).to_string())
        }
    }
}

impl From<&str> for SourceSnapshot {
    fn from(value: &str) -> Self {
        Self::from_rope(Rope::from_str(value))
    }
}

impl From<String> for SourceSnapshot {
    fn from(value: String) -> Self {
        Self::from_string(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::RopeBuilder;

    #[test]
    fn contiguous_snapshot_text_borrows_from_rope() {
        let source = SourceSnapshot::from("query Users { users { id } }");
        let text = source.full_text();
        let Some(contiguous) = source.as_contiguous_str() else {
            panic!("test source should be contiguous");
        };

        assert_eq!(text.as_ref(), "query Users { users { id } }");
        let Cow::Borrowed(borrowed) = text else {
            panic!("contiguous full text should borrow from the rope");
        };
        assert!(std::ptr::eq(borrowed.as_ptr(), contiguous.as_ptr()));
    }

    #[test]
    fn multi_chunk_snapshot_text_allocates_owned_text() {
        let chunk = "query Users { users { id } }\n";
        let expected = chunk.repeat(2048);
        let mut builder = RopeBuilder::new();
        for _ in 0..2048 {
            builder.append(chunk);
        }
        let source = SourceSnapshot::from_rope(builder.finish());
        assert!(source.as_contiguous_str().is_none());

        let text = source.full_text();

        assert_eq!(text.as_ref(), expected);
        assert!(matches!(text, Cow::Owned(_)));
    }

    #[test]
    fn byte_helpers_read_across_chunks() {
        let mut builder = RopeBuilder::new();
        builder.append("query Users {");
        builder.append(" users { id } }");
        let source = SourceSnapshot::from_rope(builder.finish());
        let split = "query Users {".len();

        assert_eq!(source.byte_at(split), Some(b' '));
        assert!(source.bytes_eq(split + 1, b"users"));
        assert!(!source.bytes_eq(split + 1, b"posts"));
    }

    #[test]
    fn text_borrows_when_range_is_inside_one_chunk() {
        let mut builder = RopeBuilder::new();
        builder.append("query Users {");
        builder.append(" users { id } }");
        let source = SourceSnapshot::from_rope(builder.finish());
        let range = TextRange::new("query ".len(), "query Users".len());

        assert!(matches!(source.text(range), Cow::Borrowed("Users")));
    }
}
