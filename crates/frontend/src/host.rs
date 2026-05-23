use crate::AnalysisResult;
use crate::completion::{CompletionItem, completions_at};
use crate::cursor::{CursorContext, cursor_context};
use crate::db::CompilerDb;
use crate::definition::{
    DefinitionResult, DefinitionTarget, SourceDefinition, SourceDefinitionKind,
    definition_target_at, find_fragment_definition,
};
use crate::document::{
    DocumentDiagnostics, DocumentFormat, DocumentKind, DocumentSnapshot, DocumentState,
    EmbeddedDocumentRegion, FileId, RevisionId, TextEdit, TextPosition, apply_text_edits,
    position_to_byte,
};
use crate::hover::{HoverInfo, hover_at};
use crate::provider::{CatalogProvider, HardcodedCatalogProvider};
use crate::semantic_tokens::{DocumentSemanticTokens, semantic_tokens_at};
use dashmap::DashMap;
use dsql_core::{
    Catalog, Diagnostic, FormatConfidence, FormattedText, LintOptions, SourceSnapshot,
};
use dsql_embedding::{RegexEmbedding, default_typescript_regex_pattern};
use ropey::Rope;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

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
        Self::with_catalog_provider(HardcodedCatalogProvider)
    }

    pub fn with_catalog_provider(provider: impl CatalogProvider) -> Self {
        let db = CompilerDb::default();
        db.set_catalog(provider.load_catalog())
            .expect("catalog should be representable by Picante");
        Self {
            inner: Arc::new(AnalysisHostInner {
                db,
                next_file: AtomicU32::new(0),
                documents: DashMap::new(),
            }),
        }
    }

    pub fn set_catalog(&self, catalog: Catalog) {
        self.inner
            .db
            .set_catalog(catalog)
            .expect("catalog should be representable by Picante");
    }

    pub fn set_lint_options(&self, options: LintOptions) {
        self.inner
            .db
            .set_lint_options(options)
            .expect("lint options should be representable by Picante");
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
            Some(document) => {
                self.clear_embedded_sources(&document.kind);
                document.file
            }
            None => self.alloc_file(),
        };
        let rope = Rope::from_str(&text);
        let revision = RevisionId(version.max(0) as u64);
        let mut state = DocumentState {
            file,
            uri: uri.clone(),
            version,
            revision,
            rope,
            kind: DocumentKind::Dsql,
        };
        self.refresh_document_analysis_sources(&mut state);
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
        {
            let mut document = self.inner.documents.get_mut(&uri)?;
            apply_text_edits(&mut document.rope, edits);
            document.version = version;
            document.revision = RevisionId(version.max(0) as u64);
            self.clear_embedded_sources(&document.kind);
            self.refresh_document_analysis_sources(&mut document);
        }
        self.document_diagnostics(&uri).await
    }

    pub async fn replace_document(
        &self,
        uri: String,
        version: i32,
        text: String,
    ) -> Option<DocumentDiagnostics> {
        let file = {
            let document = self.inner.documents.get(&uri)?;
            self.clear_embedded_sources(&document.kind);
            document.file
        };
        let rope = Rope::from_str(&text);
        let revision = RevisionId(version.max(0) as u64);
        let mut state = DocumentState {
            file,
            uri: uri.clone(),
            version,
            revision,
            rope,
            kind: DocumentKind::Dsql,
        };
        self.refresh_document_analysis_sources(&mut state);
        self.inner.documents.insert(uri.clone(), state);
        self.document_diagnostics(&uri).await
    }

    pub async fn open_document_diagnostics(&self) -> Vec<DocumentDiagnostics> {
        self.diagnostics_for_snapshots(self.open_document_snapshots())
            .await
    }

    pub fn close_document(&self, uri: &str) -> Option<DocumentSnapshot> {
        let (_, state) = self.inner.documents.remove(uri)?;
        self.inner.db.remove_source(state.file);
        self.clear_embedded_sources(&state.kind);
        Some(state.snapshot())
    }

    pub fn document_snapshot(&self, uri: &str) -> Option<DocumentSnapshot> {
        Some(self.inner.documents.get(uri)?.snapshot())
    }

    pub fn document_byte_offset(&self, uri: &str, position: TextPosition) -> Option<usize> {
        let snapshot = self.inner.documents.get(uri)?.snapshot();
        Some(position_to_byte(&snapshot.rope, position))
    }

    pub async fn document_diagnostics(&self, uri: &str) -> Option<DocumentDiagnostics> {
        let snapshot = self.inner.documents.get(uri)?.snapshot();
        self.diagnostics_for_snapshot(snapshot).await
    }

    pub async fn document_format(&self, uri: &str) -> Option<DocumentFormat> {
        let snapshot = self.inner.documents.get(uri)?.snapshot();
        if let Some(document) = self.inner.documents.get(uri)
            && let DocumentKind::Host { regions } = &document.kind
        {
            let formatted = self
                .format_embedded_document(&snapshot, regions.clone())
                .await?;
            return Some(DocumentFormat {
                snapshot,
                formatted,
            });
        }
        let formatted = self.format(snapshot.file).await?;
        Some(DocumentFormat {
            snapshot,
            formatted,
        })
    }

    pub async fn completions(
        &self,
        uri: &str,
        position: TextPosition,
    ) -> Option<Vec<CompletionItem>> {
        let snapshot = self.inner.documents.get(uri)?.snapshot();
        let byte = position_to_byte(&snapshot.rope, position);
        let Some((file, local_byte)) = self.analysis_position_at_host_byte(uri, byte) else {
            return Some(Vec::new());
        };
        let analysis = self.analyze(file).await?;
        let catalog = self.inner.db.catalog();
        let scope = self.inner.db.completion_scope(file).await.ok()?;
        Some(completions_at(
            &analysis.parse,
            &catalog,
            local_byte,
            &scope,
        ))
    }

    pub async fn completion_context_debug(
        &self,
        uri: &str,
        position: TextPosition,
    ) -> Option<String> {
        let snapshot = self.inner.documents.get(uri)?.snapshot();
        let byte = position_to_byte(&snapshot.rope, position);
        let file = self.file_at_host_byte(uri, byte)?;
        let local_byte = self.local_byte_at_host_byte(uri, byte)?;
        let analysis = self.analyze(file).await?;
        let catalog = self.inner.db.catalog();
        Some(format_completion_context(
            &cursor_context(&analysis.parse, &catalog, local_byte),
            &catalog,
        ))
    }

    pub async fn hover(&self, uri: &str, position: TextPosition) -> Option<HoverInfo> {
        let snapshot = self.inner.documents.get(uri)?.snapshot();
        let byte = position_to_byte(&snapshot.rope, position);
        let file = self.file_at_host_byte(uri, byte)?;
        let local_byte = self.local_byte_at_host_byte(uri, byte)?;
        let analysis = self.analyze(file).await?;
        let catalog = self.inner.db.catalog();
        hover_at(&analysis.parse.source_file, &catalog, local_byte)
    }

    pub async fn definition(&self, uri: &str, position: TextPosition) -> Option<DefinitionResult> {
        let snapshot = self.inner.documents.get(uri)?.snapshot();
        let byte = position_to_byte(&snapshot.rope, position);
        let file = self.file_at_host_byte(uri, byte)?;
        let local_byte = self.local_byte_at_host_byte(uri, byte)?;
        let analysis = self.analyze(file).await?;
        let catalog = self.inner.db.catalog();
        match definition_target_at(&analysis.parse.source_file, &catalog, local_byte)? {
            DefinitionTarget::Catalog(target) => Some(DefinitionResult::Catalog(target)),
            DefinitionTarget::Fragment { name } => self.find_fragment_definition(&name).await,
        }
    }

    pub async fn semantic_tokens(&self, uri: &str) -> Option<DocumentSemanticTokens> {
        let snapshot = self.inner.documents.get(uri)?.snapshot();
        if self.is_host_document(&snapshot.uri) {
            return None;
        }
        let analysis = self.analyze(snapshot.file).await?;
        let catalog = self.inner.db.catalog();
        Some(DocumentSemanticTokens {
            snapshot,
            tokens: semantic_tokens_at(&analysis.parse, &catalog),
        })
    }

    fn alloc_file(&self) -> FileId {
        FileId(self.inner.next_file.fetch_add(1, Ordering::Relaxed))
    }

    fn refresh_document_analysis_sources(&self, state: &mut DocumentState) {
        if self.is_host_document(&state.uri) {
            self.inner.db.remove_source(state.file);
            let source = state.rope.to_string();
            let embedding = RegexEmbedding::new(default_typescript_regex_pattern());
            let regions = embedding.extract(&source).unwrap_or_default();
            let mut region_states = Vec::with_capacity(regions.len());
            for region in regions {
                let file = self.alloc_file();
                self.inner
                    .db
                    .set_source_rope(file, state.revision, Rope::from_str(&region.text))
                    .ok();
                region_states.push(EmbeddedDocumentRegion::from_region(file, &region));
            }
            state.kind = DocumentKind::Host {
                regions: region_states,
            };
        } else {
            self.inner
                .db
                .set_source_rope(state.file, state.revision, state.rope.clone())
                .ok();
            state.kind = DocumentKind::Dsql;
        }
    }

    fn clear_embedded_sources(&self, kind: &DocumentKind) {
        if let DocumentKind::Host { regions } = kind {
            for region in regions {
                self.inner.db.remove_source(region.file);
            }
        }
    }

    fn analysis_position_at_host_byte(&self, uri: &str, byte: usize) -> Option<(FileId, usize)> {
        let document = self.inner.documents.get(uri)?;
        match &document.kind {
            DocumentKind::Dsql => Some((document.file, byte)),
            DocumentKind::Host { regions } => regions
                .iter()
                .find(|region| region.contains(byte))
                .map(|region| (region.file, region.local_byte(byte))),
        }
    }

    fn file_at_host_byte(&self, uri: &str, byte: usize) -> Option<FileId> {
        Some(self.analysis_position_at_host_byte(uri, byte)?.0)
    }

    fn local_byte_at_host_byte(&self, uri: &str, byte: usize) -> Option<usize> {
        Some(self.analysis_position_at_host_byte(uri, byte)?.1)
    }

    fn is_host_document(&self, uri: &str) -> bool {
        uri.ends_with(".ts") || uri.ends_with(".tsx")
    }

    fn open_document_snapshots(&self) -> Vec<DocumentSnapshot> {
        self.inner
            .documents
            .iter()
            .map(|document| document.snapshot())
            .collect()
    }

    async fn diagnostics_for_snapshots(
        &self,
        snapshots: Vec<DocumentSnapshot>,
    ) -> Vec<DocumentDiagnostics> {
        let mut results = Vec::with_capacity(snapshots.len());
        for snapshot in snapshots {
            if let Some(result) = self.diagnostics_for_snapshot(snapshot).await {
                results.push(result);
            }
        }
        results
    }

    async fn diagnostics_for_snapshot(
        &self,
        snapshot: DocumentSnapshot,
    ) -> Option<DocumentDiagnostics> {
        if let Some(document) = self.inner.documents.get(&snapshot.uri)
            && let DocumentKind::Host { regions } = &document.kind
        {
            let mut diagnostics = Vec::new();
            for region in regions {
                let mut region_diagnostics = self.inner.db.diagnostics(region.file).await.ok()?;
                for diagnostic in &mut region_diagnostics {
                    diagnostic.range.start += region.content_range.start;
                    diagnostic.range.end += region.content_range.start;
                }
                diagnostics.extend(region_diagnostics);
            }
            diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start, diagnostic.range.end));
            return Some(DocumentDiagnostics {
                snapshot,
                diagnostics,
            });
        }
        let diagnostics = self.inner.db.diagnostics(snapshot.file).await.ok()?;
        Some(DocumentDiagnostics {
            snapshot,
            diagnostics,
        })
    }

    async fn format_embedded_document(
        &self,
        snapshot: &DocumentSnapshot,
        regions: Vec<EmbeddedDocumentRegion>,
    ) -> Option<FormattedText> {
        if regions.is_empty() {
            return None;
        }

        let mut text = snapshot.rope.to_string();
        let mut diagnostics = Vec::new();
        let mut replacements = Vec::with_capacity(regions.len());

        for region in regions {
            let formatted = self.format(region.file).await?;
            diagnostics.extend(formatted.diagnostics.clone());
            replacements.push((
                region.content_range.start as usize,
                region.content_range.end as usize,
                formatted.text,
            ));
        }

        if !diagnostics.is_empty() {
            return Some(FormattedText {
                text,
                confidence: FormatConfidence::PreserveOriginal,
                diagnostics,
            });
        }

        replacements.sort_by_key(|(start, _, _)| *start);
        for (start, end, replacement) in replacements.into_iter().rev() {
            let original = &text[start..end];
            let replacement = format_embedded_replacement(original, replacement);
            text.replace_range(start..end, &replacement);
        }

        Some(FormattedText {
            text,
            confidence: FormatConfidence::Full,
            diagnostics,
        })
    }

    async fn find_fragment_definition(&self, name: &str) -> Option<DefinitionResult> {
        for snapshot in self.open_document_snapshots() {
            if let Some(analysis) = self.analyze(snapshot.file).await
                && let Some(range) = find_fragment_definition(&analysis.parse.source_file, name)
            {
                return Some(DefinitionResult::Source(SourceDefinition {
                    uri: snapshot.uri,
                    range,
                    kind: SourceDefinitionKind::Fragment,
                }));
            }
        }
        None
    }
}

fn format_completion_context(context: &CursorContext, catalog: &dsql_core::Catalog) -> String {
    match context {
        CursorContext::DocumentRoot
        | CursorContext::FragmentOnKeyword
        | CursorContext::FragmentType
        | CursorContext::RootSelection
        | CursorContext::Invalid
        | CursorContext::WhereScope
        | CursorContext::SortDirection => context.as_ref().to_string(),
        CursorContext::FragmentSpread { table }
        | CursorContext::SelectionBody { table }
        | CursorContext::ClauseList { table, used: _ }
        | CursorContext::WhereBooleanOperator { table, used: _ }
        | CursorContext::WhereColumn { table }
        | CursorContext::OrderByColumn { table } => {
            format!("{}({})", context.as_ref(), table_name(catalog, *table))
        }
        CursorContext::WhereRelationSelector { table, relation } => {
            format!(
                "{}({}, {})",
                context.as_ref(),
                table_name(catalog, *table),
                relation
            )
        }
        CursorContext::WhereOperator { data_type } => {
            format!("{}({})", context.as_ref(), data_type.as_str())
        }
    }
}

fn table_name(catalog: &dsql_core::Catalog, table: dsql_core::TableId) -> String {
    catalog
        .tables
        .iter()
        .find(|candidate| candidate.id == table)
        .map(|candidate| candidate.name.clone())
        .unwrap_or_else(|| format!("table#{}", table.0))
}

fn format_embedded_replacement(original: &str, mut formatted: String) -> String {
    if original.starts_with('\n') && !formatted.starts_with('\n') {
        formatted.insert(0, '\n');
    }
    if original.ends_with('\n') && !formatted.ends_with('\n') {
        formatted.push('\n');
    }
    formatted
}
