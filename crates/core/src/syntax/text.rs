use facet::Facet;
use ropey::Rope;
use std::borrow::Cow;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

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

/// Immutable source text for one document revision.
///
/// The Rope is the canonical editing and source-query representation. The
/// full-text cache is populated only when a caller needs contiguous document
/// text and the Rope cannot satisfy that request directly.
#[derive(Clone, Debug, Facet)]
#[facet(opaque, traits(Debug))]
pub struct SourceDocument {
    inner: Arc<SourceDocumentInner>,
}

#[derive(Debug)]
struct SourceDocumentInner {
    rope: Arc<Rope>,
    full_text: OnceLock<Arc<str>>,
}

impl SourceDocument {
    /// Creates a document revision from contiguous source text.
    pub fn from_string(text: String) -> Self {
        crate::debug::record_document_from_string(text.len());
        let text: Arc<str> = Arc::from(text);
        let full_text = OnceLock::new();
        let _ = full_text.set(text.clone());
        Self {
            inner: Arc::new(SourceDocumentInner {
                rope: Arc::new(Rope::from_str(&text)),
                full_text,
            }),
        }
    }

    /// Creates a document revision from a Rope.
    pub fn from_rope(rope: Rope) -> Self {
        crate::debug::record_document_from_rope(rope.len());
        Self {
            inner: Arc::new(SourceDocumentInner {
                rope: Arc::new(rope),
                full_text: OnceLock::new(),
            }),
        }
    }

    /// Creates a document revision from a shared Rope.
    pub fn from_arc_rope(rope: Arc<Rope>) -> Self {
        crate::debug::record_document_from_rope(rope.len());
        Self {
            inner: Arc::new(SourceDocumentInner {
                rope,
                full_text: OnceLock::new(),
            }),
        }
    }

    /// Returns the Rope for this document revision.
    pub fn as_rope(&self) -> &Rope {
        &self.inner.rope
    }

    /// Returns a shared Rope handle for this document revision.
    pub fn arc_rope(&self) -> Arc<Rope> {
        self.inner.rope.clone()
    }

    /// Returns the document length in bytes.
    pub fn len_bytes(&self) -> usize {
        self.as_rope().len()
    }

    /// Returns the whole document as a contiguous source view.
    ///
    /// The returned `&str` borrows from the document cache when available,
    /// borrows directly from the Rope when the document is contiguous, and
    /// otherwise materializes the document once into the cache.
    pub fn source_view(&self) -> &str {
        if let Some(text) = self.cached_source_view() {
            text
        } else if let Some(text) = self.contiguous_text(TextRange::new(0, self.len_bytes())) {
            text
        } else {
            self.materialized_source_view()
        }
    }

    fn materialized_source_view(&self) -> &str {
        self.inner
            .full_text
            .get_or_init(|| {
                let text = self.as_rope().to_string();
                crate::debug::record_full_text_materialization(text.len());
                Arc::from(text)
            })
            .as_ref()
    }

    fn cached_source_view(&self) -> Option<&str> {
        self.inner.full_text.get().map(|text| text.as_ref())
    }

    fn source_range_view(&self, range: TextRange) -> &str {
        if let Some(text) = self.cached_source_view() {
            &text[range.as_usize()]
        } else if let Some(text) = self.contiguous_text(range) {
            text
        } else {
            &self.materialized_source_view()[range.as_usize()]
        }
    }

    fn contiguous_text(&self, range: TextRange) -> Option<&str> {
        if range.end > self.len_bytes() as u32 {
            return None;
        }
        if range.is_empty() {
            return Some("");
        }
        let absolute = range.as_usize();
        let (mut chunks, chunk_start) = self.as_rope().chunks_at(absolute.start);
        let chunk = chunks.next()?;
        let start = absolute.start - chunk_start;
        let end = start.checked_add(absolute.len())?;
        chunk.get(start..end)
    }

    /// Returns a source region covering the whole document.
    pub fn full_region(&self) -> SourceRegion {
        SourceRegion::new(self.clone(), TextRange::new(0, self.len_bytes()), 0)
    }
}

/// A compiler source unit represented as a byte range inside a document.
#[derive(Clone, Debug, Facet)]
#[facet(opaque, traits(Debug))]
pub struct SourceRegion {
    document: SourceDocument,
    content_range: TextRange,
    source_offset: u32,
}

impl SourceRegion {
    /// Creates a source region over a document revision.
    pub fn new(document: SourceDocument, content_range: TextRange, source_offset: u32) -> Self {
        Self {
            document,
            content_range,
            source_offset,
        }
    }

    /// Returns the document revision this region points into.
    pub fn document(&self) -> &SourceDocument {
        &self.document
    }

    /// Returns the byte range of this region inside the document.
    pub fn content_range(&self) -> TextRange {
        self.content_range
    }

    /// Returns the byte offset used to map local ranges back to the document.
    pub fn source_offset(&self) -> u32 {
        self.source_offset
    }

    /// Returns the region length in bytes.
    pub fn len_bytes(&self) -> usize {
        self.content_range.len()
    }
}

/// Immutable source text snapshot used by compiler stages.
#[derive(Clone, Debug)]
pub struct SourceSnapshot {
    region: SourceRegion,
}

impl SourceSnapshot {
    /// Creates a full-document source snapshot from contiguous source text.
    pub fn from_string(text: String) -> Self {
        Self::from_document(SourceDocument::from_string(text))
    }

    /// Creates a full-document source snapshot from a Rope.
    pub fn from_rope(rope: Rope) -> Self {
        Self::from_document(SourceDocument::from_rope(rope))
    }

    /// Creates a full-document source snapshot from a shared Rope.
    pub fn from_arc_rope(rope: Arc<Rope>) -> Self {
        Self::from_document(SourceDocument::from_arc_rope(rope))
    }

    /// Creates a full-document source snapshot from a document revision.
    pub fn from_document(document: SourceDocument) -> Self {
        Self {
            region: document.full_region(),
        }
    }

    /// Creates a source snapshot from an existing source region.
    pub fn from_region(region: SourceRegion) -> Self {
        Self { region }
    }

    /// Returns the region represented by this snapshot.
    pub fn region(&self) -> &SourceRegion {
        &self.region
    }

    fn document_rope(&self) -> &Rope {
        self.region.document().as_rope()
    }

    /// Returns a shared Rope for this snapshot's region.
    pub fn arc_rope(&self) -> Arc<Rope> {
        if self.region.content_range.start == 0
            && self.region.content_range.end as usize == self.document_rope().len()
        {
            self.region.document().arc_rope()
        } else {
            Arc::new(self.to_rope())
        }
    }

    fn absolute_range(&self, range: TextRange) -> Range<usize> {
        let start = self.region.content_range.start as usize + range.start as usize;
        let end = self.region.content_range.start as usize + range.end as usize;
        start..end
    }

    /// Returns this snapshot as `&str` when its full region is one Rope chunk.
    pub fn as_contiguous_str(&self) -> Option<&str> {
        self.contiguous_text(TextRange::new(0, self.len_bytes()))
    }

    /// Returns this source unit as a contiguous string view.
    ///
    /// The returned `&str` borrows from the document cache when available,
    /// borrows directly from the Rope when this region is contiguous, and
    /// otherwise materializes the full document once into the document cache
    /// before returning this region.
    pub fn source_view(&self) -> &str {
        self.region
            .document()
            .source_range_view(self.region.content_range)
    }

    /// Returns this snapshot's full region as a Rope.
    pub fn to_rope(&self) -> Rope {
        crate::debug::record_region_rope_materialization(self.len_bytes());
        Rope::from(
            self.document_rope()
                .slice(self.region.content_range.as_usize()),
        )
    }

    /// Consumes the snapshot and returns its region as a Rope.
    pub fn into_rope(self) -> Rope {
        self.to_rope()
    }

    /// Returns the snapshot length in bytes.
    pub fn len_bytes(&self) -> usize {
        self.region.len_bytes()
    }

    /// Returns the byte at a local source offset.
    pub fn byte_at(&self, byte: usize) -> Option<u8> {
        if byte >= self.len_bytes() {
            return None;
        }
        self.document_rope()
            .bytes_at(self.region.content_range.start as usize + byte)
            .next()
    }

    /// Returns whether bytes at a local source offset equal `expected`.
    pub fn bytes_eq(&self, start: usize, expected: &[u8]) -> bool {
        let Some(end) = start.checked_add(expected.len()) else {
            return false;
        };
        if end > self.len_bytes() {
            return false;
        }
        self.document_rope()
            .bytes_at(self.region.content_range.start as usize + start)
            .take(expected.len())
            .eq(expected.iter().copied())
    }

    /// Returns text for `range` if it is contained in one Rope chunk.
    pub fn contiguous_text(&self, range: TextRange) -> Option<&str> {
        if range.end > self.len_bytes() as u32 {
            return None;
        }
        if range.is_empty() {
            return Some("");
        }
        self.region.document().contiguous_text(TextRange::new(
            self.region.content_range.start as usize + range.start as usize,
            self.region.content_range.start as usize + range.end as usize,
        ))
    }

    /// Returns text for a local source range.
    pub fn text(&self, range: TextRange) -> Cow<'_, str> {
        if let Some(text) = self.contiguous_text(range) {
            Cow::Borrowed(text)
        } else {
            let text = self
                .document_rope()
                .slice(self.absolute_range(range))
                .to_string();
            crate::debug::record_range_text_materialization(text.len());
            Cow::Owned(text)
        }
    }
}

impl From<SourceRegion> for SourceSnapshot {
    fn from(region: SourceRegion) -> Self {
        Self::from_region(region)
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
        let text = source.source_view();
        let Some(contiguous) = source.as_contiguous_str() else {
            panic!("test source should be contiguous");
        };

        assert_eq!(text, "query Users { users { id } }");
        assert!(std::ptr::eq(text.as_ptr(), contiguous.as_ptr()));
    }

    #[test]
    fn multi_chunk_snapshot_text_materializes_document_cache() {
        let chunk = "query Users { users { id } }\n";
        let expected = chunk.repeat(2048);
        let mut builder = RopeBuilder::new();
        for _ in 0..2048 {
            builder.append(chunk);
        }
        let source = SourceSnapshot::from_rope(builder.finish());
        assert!(source.as_contiguous_str().is_none());

        let text = source.source_view();

        assert_eq!(text, expected);
        assert!(source.region().document().cached_source_view().is_some());
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

    #[test]
    fn region_snapshot_uses_local_byte_offsets() {
        let document = SourceDocument::from_string("prefix query Users { users { id } }".into());
        let start = "prefix ".len();
        let region = SourceRegion::new(
            document,
            TextRange::new(start, "prefix query Users".len()),
            start as u32,
        );
        let source = SourceSnapshot::from_region(region);

        assert_eq!(source.source_view(), "query Users");
        assert_eq!(source.byte_at(0), Some(b'q'));
        assert!(source.bytes_eq("query ".len(), b"Users"));
    }

    #[test]
    fn region_full_text_borrows_from_document_cache() {
        let chunk = "prefix query Users { users { id } }\n";
        let mut builder = RopeBuilder::new();
        for _ in 0..2048 {
            builder.append(chunk);
        }
        let document = SourceDocument::from_rope(builder.finish());
        let document_text = document.source_view();
        let start = "prefix ".len();
        let end = "prefix query Users".len();
        let source = SourceSnapshot::from_region(SourceRegion::new(
            document.clone(),
            TextRange::new(start, end),
            start as u32,
        ));

        let text = source.source_view();

        assert_eq!(text, "query Users");
        assert!(std::ptr::eq(
            text.as_ptr(),
            document_text[start..end].as_ptr()
        ));
    }
}
