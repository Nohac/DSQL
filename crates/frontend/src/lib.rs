use dashmap::DashMap;
use dsql_core::{
    CheckedFile, Diagnostic, FormattedText, Interner, LoweredFile, ParseResult, SourceFile,
    SourceSnapshot, SyntaxTree, check_file, format_file, lower_file, parse_source,
};
use facet::Facet;
use picante::PicanteResult;
use ropey::{LineType, Rope};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Facet)]
pub struct FileId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Facet)]
pub struct RevisionId(pub u64);

#[derive(Clone, Debug)]
pub struct AnalysisResult {
    pub parse: ParseResult,
    pub lower: LoweredFile,
    pub check: CheckedFile,
}

#[derive(Clone, Debug, Facet)]
pub struct ParsedFile {
    pub tree: SyntaxTree,
    pub source_file: SourceFile,
    pub diagnostics: Vec<Diagnostic>,
}

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
struct DocumentState {
    file: FileId,
    uri: String,
    version: i32,
    revision: RevisionId,
    rope: Rope,
}

impl DocumentState {
    fn snapshot(&self) -> DocumentSnapshot {
        DocumentSnapshot {
            file: self.file,
            uri: self.uri.clone(),
            version: self.version,
            revision: self.revision,
            rope: self.rope.clone(),
        }
    }
}

#[picante::input]
pub struct SourceInput {
    #[key]
    pub file: FileId,
    pub revision: RevisionId,
    pub text: Arc<str>,
}

#[picante::tracked]
pub async fn parse_file<DB: CompilerDatabaseTrait>(
    db: &DB,
    source: SourceInput,
) -> PicanteResult<ParsedFile> {
    let text = source.text(db)?;
    let parse = parse_source(SourceSnapshot::from_arc(text));
    Ok(ParsedFile {
        tree: parse.tree,
        source_file: parse.source_file,
        diagnostics: parse.diagnostics,
    })
}

#[picante::tracked]
pub async fn lower_file_query<DB: CompilerDatabaseTrait>(
    db: &DB,
    source: SourceInput,
) -> PicanteResult<LoweredFile> {
    let parse = parse_file(db, source).await?;
    let mut interner = Interner::default();
    Ok(lower_file(&parse.source_file, &mut interner))
}

#[picante::tracked]
pub async fn check_file_query<DB: CompilerDatabaseTrait>(
    db: &DB,
    source: SourceInput,
) -> PicanteResult<CheckedFile> {
    let parse = parse_file(db, source).await?;
    Ok(check_file(&parse.source_file))
}

#[picante::tracked]
pub async fn diagnostics_for_file<DB: CompilerDatabaseTrait>(
    db: &DB,
    source: SourceInput,
) -> PicanteResult<Vec<Diagnostic>> {
    let parse = parse_file(db, source).await?;
    let lower = lower_file_query(db, source).await?;
    let check = check_file_query(db, source).await?;
    Ok(collect_diagnostics_parts(
        &parse.diagnostics,
        &lower.diagnostics,
        &check.diagnostics,
    ))
}

#[picante::tracked]
pub async fn formatted_text_for_file<DB: CompilerDatabaseTrait>(
    db: &DB,
    source: SourceInput,
) -> PicanteResult<Option<String>> {
    let parsed = parse_file(db, source).await?;
    let text = source.text(db)?;
    let parse = ParseResult {
        source: SourceSnapshot::from_arc(text),
        tree: parsed.tree,
        source_file: parsed.source_file,
        diagnostics: parsed.diagnostics,
    };
    let formatted = format_file(&parse);
    if formatted.diagnostics.is_empty() {
        Ok(Some(formatted.text))
    } else {
        Ok(None)
    }
}

#[picante::db(
    inputs(SourceInput),
    tracked(
        parse_file,
        lower_file_query,
        check_file_query,
        diagnostics_for_file,
        formatted_text_for_file
    ),
    db_trait(CompilerDatabaseTrait)
)]
pub struct CompilerDb {
    source_inputs: DashMap<FileId, SourceInput>,
}

impl Default for CompilerDb {
    fn default() -> Self {
        Self::new(DashMap::new())
    }
}

impl CompilerDb {
    fn set_source_rope(&self, file: FileId, revision: RevisionId, rope: Rope) -> PicanteResult<()> {
        let text = Arc::<str>::from(rope.to_string());
        let source = SourceInput::new(self, file, revision, text)?;
        self.source_inputs.insert(file, source);
        Ok(())
    }

    fn remove_source(&self, file: FileId) {
        self.source_inputs.remove(&file);
    }

    fn source_input(&self, file: FileId) -> Option<SourceInput> {
        self.source_inputs.get(&file).map(|source| *source)
    }

    async fn analysis(&self, file: FileId) -> Option<AnalysisResult> {
        let source = self.source_input(file)?;
        let parsed = parse_file(self, source).await.ok()?;
        let lower = lower_file_query(self, source).await.ok()?;
        let check = check_file_query(self, source).await.ok()?;
        let text = source.text(self).ok()?;
        Some(AnalysisResult {
            parse: ParseResult {
                source: SourceSnapshot::from_arc(text),
                tree: parsed.tree,
                source_file: parsed.source_file,
                diagnostics: parsed.diagnostics,
            },
            lower,
            check,
        })
    }

    async fn diagnostics(&self, file: FileId) -> PicanteResult<Vec<Diagnostic>> {
        let Some(source) = self.source_input(file) else {
            return Ok(Vec::new());
        };
        diagnostics_for_file(self, source).await
    }

    async fn formatted_text(&self, file: FileId) -> PicanteResult<Option<String>> {
        let Some(source) = self.source_input(file) else {
            return Ok(None);
        };
        formatted_text_for_file(self, source).await
    }
}

#[derive(Clone)]
pub struct AnalysisHost {
    inner: Arc<AnalysisHostInner>,
}

struct AnalysisHostInner {
    db: CompilerDb,
    next_file: AtomicU32,
    documents: DashMap<String, DocumentState>,
}

impl Default for AnalysisHost {
    fn default() -> Self {
        Self::new()
    }
}

impl AnalysisHost {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AnalysisHostInner {
                db: CompilerDb::default(),
                next_file: AtomicU32::new(0),
                documents: DashMap::new(),
            }),
        }
    }

    pub fn create_file(&self, source: SourceSnapshot) -> FileId {
        let id = self.alloc_file();
        self.inner
            .db
            .set_source_rope(id, RevisionId(0), source.into_rope())
            .expect("source input should be representable by Picante");
        id
    }

    pub fn set_file_source(&self, file: FileId, revision: RevisionId, source: SourceSnapshot) {
        self.inner
            .db
            .set_source_rope(file, revision, source.into_rope())
            .expect("source input should be representable by Picante");
    }

    pub async fn analyze(&self, file: FileId) -> Option<AnalysisResult> {
        self.inner.db.analysis(file).await
    }

    pub async fn diagnostics(&self, file: FileId) -> Option<Vec<Diagnostic>> {
        self.inner.db.diagnostics(file).await.ok()
    }

    pub async fn format(&self, file: FileId) -> Option<FormattedText> {
        let text = self.inner.db.formatted_text(file).await.ok()??;
        Some(FormattedText {
            text,
            confidence: dsql_core::FormatConfidence::Full,
            diagnostics: Vec::new(),
        })
    }

    pub async fn open_document(
        &self,
        uri: String,
        version: i32,
        text: String,
    ) -> DocumentDiagnostics {
        let file = match self.inner.documents.get(&uri) {
            Some(document) => document.file,
            None => self.alloc_file(),
        };
        let rope = Rope::from_str(&text);
        let revision = RevisionId(version.max(0) as u64);
        self.inner
            .db
            .set_source_rope(file, revision, rope.clone())
            .ok();
        let state = DocumentState {
            file,
            uri: uri.clone(),
            version,
            revision,
            rope,
        };
        let snapshot = state.snapshot();
        self.inner.documents.insert(uri.clone(), state);
        self.document_diagnostics(&uri)
            .await
            .unwrap_or_else(|| DocumentDiagnostics {
                snapshot,
                diagnostics: Vec::new(),
            })
    }

    pub async fn change_document(
        &self,
        uri: String,
        version: i32,
        edits: Vec<TextEdit>,
    ) -> Option<DocumentDiagnostics> {
        let snapshot = {
            let mut document = self.inner.documents.get_mut(&uri)?;
            apply_text_edits(&mut document.rope, edits);
            document.version = version;
            document.revision = RevisionId(version.max(0) as u64);
            document.snapshot()
        };
        self.inner
            .db
            .set_source_rope(snapshot.file, snapshot.revision, snapshot.rope.clone())
            .ok()?;
        self.document_diagnostics(&uri).await
    }

    pub async fn replace_document(
        &self,
        uri: String,
        version: i32,
        text: String,
    ) -> Option<DocumentDiagnostics> {
        let file = self.inner.documents.get(&uri)?.file;
        let rope = Rope::from_str(&text);
        let revision = RevisionId(version.max(0) as u64);
        self.inner
            .db
            .set_source_rope(file, revision, rope.clone())
            .ok()?;
        let state = DocumentState {
            file,
            uri: uri.clone(),
            version,
            revision,
            rope,
        };
        self.inner.documents.insert(uri.clone(), state);
        self.document_diagnostics(&uri).await
    }

    pub fn close_document(&self, uri: &str) -> Option<DocumentSnapshot> {
        let (_, state) = self.inner.documents.remove(uri)?;
        self.inner.db.remove_source(state.file);
        Some(state.snapshot())
    }

    pub async fn document_diagnostics(&self, uri: &str) -> Option<DocumentDiagnostics> {
        let snapshot = self.inner.documents.get(uri)?.snapshot();
        let diagnostics = self.inner.db.diagnostics(snapshot.file).await.ok()?;
        Some(DocumentDiagnostics {
            snapshot,
            diagnostics,
        })
    }

    pub async fn document_format(&self, uri: &str) -> Option<DocumentFormat> {
        let snapshot = self.inner.documents.get(uri)?.snapshot();
        let formatted = self.format(snapshot.file).await?;
        Some(DocumentFormat {
            snapshot,
            formatted,
        })
    }

    fn alloc_file(&self) -> FileId {
        FileId(self.inner.next_file.fetch_add(1, Ordering::Relaxed))
    }
}

fn apply_text_edits(rope: &mut Rope, edits: Vec<TextEdit>) {
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

fn position_to_byte(rope: &Rope, position: TextPosition) -> usize {
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

pub fn analyze_source(source: SourceSnapshot) -> AnalysisResult {
    let parse = parse_source(source);
    let mut interner = Interner::default();
    let lower = lower_file(&parse.source_file, &mut interner);
    let check = check_file(&parse.source_file);
    AnalysisResult {
        parse,
        lower,
        check,
    }
}

pub fn collect_diagnostics(analysis: &AnalysisResult) -> Vec<Diagnostic> {
    collect_diagnostics_parts(
        &analysis.parse.diagnostics,
        &analysis.lower.diagnostics,
        &analysis.check.diagnostics,
    )
}

fn collect_diagnostics_parts(
    parse: &[Diagnostic],
    lower: &[Diagnostic],
    check: &[Diagnostic],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    diagnostics.extend(parse.iter().cloned());
    diagnostics.extend(lower.iter().cloned());
    diagnostics.extend(check.iter().cloned());
    diagnostics.sort_by_key(|diag| (diag.range.start, diag.range.end));
    diagnostics
}
