use dsql_core::{Diagnostic, FormattedText};
use dsql_embedding::EmbeddedRegion;
use facet::Facet;
use ropey::{LineType, Rope};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
pub struct FileId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Facet)]
pub struct RevisionId(pub u64);

#[derive(Clone, Debug)]
pub struct DocumentSnapshot {
    pub file: FileId,
    pub uri: String,
    pub version: i32,
    pub revision: RevisionId,
    pub rope: Rope,
}

#[derive(Clone, Debug)]
pub struct DocumentDiagnostics {
    pub snapshot: DocumentSnapshot,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug)]
pub struct DocumentFormat {
    pub snapshot: DocumentSnapshot,
    pub formatted: FormattedText,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextEditRange {
    pub start: TextPosition,
    pub end: TextPosition,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Option<TextEditRange>,
    pub text: String,
}

#[derive(Clone, Debug)]
pub(crate) struct DocumentState {
    pub(crate) file: FileId,
    pub(crate) uri: String,
    pub(crate) version: i32,
    pub(crate) revision: RevisionId,
    pub(crate) rope: Rope,
    pub(crate) kind: DocumentKind,
}

impl DocumentState {
    pub(crate) fn snapshot(&self) -> DocumentSnapshot {
        DocumentSnapshot {
            file: self.file,
            uri: self.uri.clone(),
            version: self.version,
            revision: self.revision,
            rope: self.rope.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum DocumentKind {
    Dsql,
    Host {
        regions: Vec<EmbeddedDocumentRegion>,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct EmbeddedDocumentRegion {
    pub(crate) file: FileId,
    pub(crate) content_range: dsql_core::TextRange,
}

impl EmbeddedDocumentRegion {
    pub(crate) fn from_region(file: FileId, region: &EmbeddedRegion) -> Self {
        Self {
            file,
            content_range: region.content_range,
        }
    }

    pub(crate) fn contains(&self, byte: usize) -> bool {
        byte >= self.content_range.start as usize && byte <= self.content_range.end as usize
    }

    pub(crate) fn local_byte(&self, byte: usize) -> usize {
        byte.saturating_sub(self.content_range.start as usize)
    }

    pub(crate) fn host_range(&self, range: dsql_core::TextRange) -> dsql_core::TextRange {
        dsql_core::TextRange::new(
            self.content_range.start as usize + range.start as usize,
            self.content_range.start as usize + range.end as usize,
        )
    }
}

pub(crate) fn apply_text_edits(rope: &mut Rope, edits: Vec<TextEdit>) {
    for edit in edits {
        if let Some(range) = edit.range {
            let start = position_to_byte(rope, range.start);
            let end = position_to_byte(rope, range.end);
            rope.remove(start..end);
            rope.insert(start, &edit.text);
        } else {
            *rope = Rope::from_str(&edit.text);
        }
    }
}

pub(crate) fn position_to_byte(rope: &Rope, position: TextPosition) -> usize {
    let line_count = rope.len_lines(LineType::LF_CR);
    let line = (position.line as usize).min(line_count.saturating_sub(1));
    let line_start = rope.line_to_byte_idx(line, LineType::LF_CR);
    let line_end = if line + 1 < line_count {
        rope.line_to_byte_idx(line + 1, LineType::LF_CR)
    } else {
        rope.len()
    };
    let line_slice = rope.slice(line_start..line_end);
    let utf16 = (position.character as usize).min(line_slice.len_utf16());
    line_start + line_slice.utf16_to_byte_idx(utf16)
}
