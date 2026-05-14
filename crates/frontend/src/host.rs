use crate::AnalysisResult;
use crate::completion::{CompletionItem, completions_at};
use crate::db::CompilerDb;
use crate::definition::{
    DefinitionResult, DefinitionTarget, SourceDefinition, SourceDefinitionKind,
    definition_target_at, find_fragment_definition,
};
use crate::document::{
    DocumentDiagnostics, DocumentFormat, DocumentSnapshot, DocumentState, FileId, RevisionId,
    TextEdit, TextPosition, apply_text_edits, position_to_byte,
};
use crate::hover::{HoverInfo, hover_at};
use crate::provider::{CatalogProvider, HardcodedCatalogProvider};
use crate::semantic_tokens::{DocumentSemanticTokens, semantic_tokens_at};
use dashmap::DashMap;
use dsql_core::{Catalog, Diagnostic, FormattedText, SourceSnapshot};
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

    pub async fn open_document_diagnostics(&self) -> Vec<DocumentDiagnostics> {
        self.diagnostics_for_snapshots(self.open_document_snapshots())
            .await
    }

    pub fn close_document(&self, uri: &str) -> Option<DocumentSnapshot> {
        let (_, state) = self.inner.documents.remove(uri)?;
        self.inner.db.remove_source(state.file);
        Some(state.snapshot())
    }

    pub fn document_snapshot(&self, uri: &str) -> Option<DocumentSnapshot> {
        Some(self.inner.documents.get(uri)?.snapshot())
    }

    pub async fn document_diagnostics(&self, uri: &str) -> Option<DocumentDiagnostics> {
        let snapshot = self.inner.documents.get(uri)?.snapshot();
        self.diagnostics_for_snapshot(snapshot).await
    }

    pub async fn document_format(&self, uri: &str) -> Option<DocumentFormat> {
        let snapshot = self.inner.documents.get(uri)?.snapshot();
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
        let analysis = self.analyze(snapshot.file).await?;
        let catalog = self.inner.db.catalog();
        Some(completions_at(&analysis.parse, &catalog, byte))
    }

    pub async fn hover(&self, uri: &str, position: TextPosition) -> Option<HoverInfo> {
        let snapshot = self.inner.documents.get(uri)?.snapshot();
        let byte = position_to_byte(&snapshot.rope, position);
        let analysis = self.analyze(snapshot.file).await?;
        let catalog = self.inner.db.catalog();
        hover_at(&analysis.parse.source_file, &catalog, byte)
    }

    pub async fn definition(&self, uri: &str, position: TextPosition) -> Option<DefinitionResult> {
        let snapshot = self.inner.documents.get(uri)?.snapshot();
        let byte = position_to_byte(&snapshot.rope, position);
        let analysis = self.analyze(snapshot.file).await?;
        let catalog = self.inner.db.catalog();
        match definition_target_at(&analysis.parse.source_file, &catalog, byte)? {
            DefinitionTarget::Catalog(target) => Some(DefinitionResult::Catalog(target)),
            DefinitionTarget::Fragment { name } => self.find_fragment_definition(&name).await,
        }
    }

    pub async fn semantic_tokens(&self, uri: &str) -> Option<DocumentSemanticTokens> {
        let snapshot = self.inner.documents.get(uri)?.snapshot();
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
        let diagnostics = self.inner.db.diagnostics(snapshot.file).await.ok()?;
        Some(DocumentDiagnostics {
            snapshot,
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
