use crate::{
    AnalysisContext, AnalysisContextId, AnalysisHost, CompletionItem, DefinitionResult, FileId,
    HoverInfo, RevisionId,
    completion::completions_at,
    cursor::cursor_context,
    definition::{DefinitionTarget, SourceDefinition, SourceDefinitionKind, definition_target_at},
    document::{
        DocumentFormat, DocumentSnapshot, TextEdit, TextPosition, apply_text_edits,
        position_to_byte,
    },
    hover::hover_at,
    semantic_tokens::{DocumentSemanticTokens, semantic_tokens_at},
};
use dashmap::DashMap;
use dsql_core::{
    DefinitionRecord, Diagnostic, DiagnosticCode, DiagnosticSource, FormatConfidence,
    FormattedText, FragmentMap, Severity, SourceSnapshot, TextRange, VariableBinding,
    infer_fragment_variable_bindings, infer_query_variable_bindings,
};
use dsql_embedding::{RegexEmbedding, default_typescript_regex_pattern};
use ropey::{LineType, Rope};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Stable identity for a physical source document known to project analysis.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhysicalDocumentId(pub PathBuf);

/// Metadata for a file or editor buffer whose source is stored in `ProjectSourceDb`.
#[derive(Clone, Debug)]
pub struct PhysicalDocument {
    pub id: PhysicalDocumentId,
    pub path: Option<PathBuf>,
    pub revision: RevisionId,
    pub residency: SourceResidency,
}

/// Source lifetime category for a physical document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceResidency {
    AnalysisSnapshot,
    OpenEditable,
}

/// Rope-backed source state owned by project analysis.
#[derive(Clone, Debug)]
pub struct SourceEntry {
    pub id: PhysicalDocumentId,
    pub path: Option<PathBuf>,
    pub revision: RevisionId,
    pub rope: Rope,
    pub residency: SourceResidency,
}

/// Project-level source store for physical documents and live editor buffers.
#[derive(Clone)]
pub struct ProjectSourceDb {
    inner: Arc<ProjectSourceDbInner>,
}

struct ProjectSourceDbInner {
    entries: DashMap<PhysicalDocumentId, SourceEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProjectSourceRegion {
    pub physical_document: PhysicalDocumentId,
    pub content_range: TextRange,
    pub source_offset: u32,
}

#[derive(Clone, Debug)]
pub struct DocumentBundle {
    pub context: AnalysisContext,
    pub regions: Vec<ProjectSourceRegion>,
}

/// A source region published into the shared analysis host for one context.
///
/// The `file` is the shared analysis source identity. Multiple contexts can
/// reference the same `file` when they import the same physical source region.
#[derive(Clone, Debug)]
pub struct ProjectContextSource {
    pub context: AnalysisContextId,
    pub physical_document: PhysicalDocumentId,
    pub file: FileId,
    pub content_range: TextRange,
    pub source_offset: u32,
    pub resolution_scope: String,
}

/// Project-level analysis handle for source ownership and context routing.
///
/// Cloning this handle is cheap and shares the same source DB, shared analysis
/// host, context source sets, and source-region allocation cache.
#[derive(Clone)]
pub struct ProjectAnalysis {
    inner: Arc<ProjectAnalysisInner>,
}

struct ProjectAnalysisInner {
    sources: ProjectSourceDb,
    analysis: AnalysisHost,
    contexts: DashMap<AnalysisContextId, Arc<AnalysisContextState>>,
    source_files: DashMap<ProjectSourceRegion, FileId>,
    source_scopes: DashMap<ProjectSourceRegion, String>,
    regions_by_scope: DashMap<String, Vec<ProjectSourceRegion>>,
    effective_contexts: DashMap<String, EffectiveResolutionContext>,
}

#[derive(Clone, Debug)]
struct AnalysisContextState {
    bundle: DocumentBundle,
    sources: Vec<ProjectContextSource>,
}

#[derive(Clone, Debug)]
pub struct ProjectDiagnostic {
    pub context: AnalysisContextId,
    pub physical_document: PhysicalDocumentId,
    pub source_offset: u32,
    pub diagnostic: Diagnostic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourcePosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresentedDiagnostic {
    pub context: Option<AnalysisContext>,
    pub physical_document: PhysicalDocumentId,
    pub path: Option<PathBuf>,
    pub source_offset: u32,
    pub embedded_range: TextRange,
    pub range: TextRange,
    pub start_position: SourcePosition,
    pub end_position: SourcePosition,
    pub diagnostic: Diagnostic,
}

#[derive(Clone, Debug)]
pub struct ProjectSourceScope {
    pub physical_document: PhysicalDocumentId,
    pub path: Option<PathBuf>,
    pub source_offset: u32,
    pub resolution_scope: String,
}

#[derive(Clone, Debug)]
pub struct ProjectGenerationModel {
    pub document_count: usize,
    pub query_count: usize,
    pub contexts: Vec<ProjectGenerationContext>,
    pub diagnostics: Vec<PresentedDiagnostic>,
    pub source_scopes: Vec<ProjectSourceScope>,
}

#[derive(Clone, Debug)]
pub struct ProjectGenerationContext {
    pub context: AnalysisContext,
    pub definitions: Vec<ProjectGenerationDefinition>,
}

#[derive(Clone, Debug)]
pub struct ProjectGenerationDefinition {
    pub physical_document: PhysicalDocumentId,
    pub path: Option<PathBuf>,
    pub file: FileId,
    pub source_offset: u32,
    pub resolution_scope: String,
    pub definition: DefinitionRecord,
}

#[derive(Clone, Debug)]
struct EffectiveResolutionContext {
    name: String,
    scopes: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VisitState {
    Visiting,
    Visited,
}

impl Default for ProjectSourceDb {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectSourceDb {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ProjectSourceDbInner {
                entries: DashMap::new(),
            }),
        }
    }

    pub fn insert(&self, entry: SourceEntry) {
        self.inner.entries.insert(entry.id.clone(), entry);
    }

    pub fn load_analysis_snapshot(
        &self,
        path: impl AsRef<Path>,
    ) -> dsql_project::Result<PhysicalDocumentId> {
        let path = path.as_ref();
        let id = PhysicalDocumentId(path.to_path_buf());
        if self.inner.entries.contains_key(&id) {
            return Ok(id);
        }
        let file = File::open(path).map_err(|source| dsql_project::ProjectError::ReadFile {
            path: path.to_path_buf(),
            source,
        })?;
        let rope =
            Rope::from_reader(file).map_err(|source| dsql_project::ProjectError::ReadFile {
                path: path.to_path_buf(),
                source,
            })?;
        self.insert(SourceEntry {
            id: id.clone(),
            path: Some(path.to_path_buf()),
            revision: file_revision(path).unwrap_or_default(),
            rope,
            residency: SourceResidency::AnalysisSnapshot,
        });
        Ok(id)
    }

    pub fn physical_document(&self, document_id: &PhysicalDocumentId) -> Option<PhysicalDocument> {
        self.inner
            .entries
            .get(document_id)
            .map(|entry| PhysicalDocument {
                id: entry.id.clone(),
                path: entry.path.clone(),
                revision: entry.revision,
                residency: entry.residency,
            })
    }

    pub fn source(&self, document_id: &PhysicalDocumentId) -> Option<SourceEntry> {
        self.inner
            .entries
            .get(document_id)
            .map(|entry| entry.clone())
    }

    pub fn documents_with_residency(&self, residency: SourceResidency) -> Vec<PhysicalDocumentId> {
        let mut documents = self
            .inner
            .entries
            .iter()
            .filter_map(|entry| (entry.residency == residency).then(|| entry.id.clone()))
            .collect::<Vec<_>>();
        documents.sort();
        documents
    }

    pub fn open_editable(
        &self,
        id: PhysicalDocumentId,
        path: Option<PathBuf>,
        revision: RevisionId,
        text: String,
    ) {
        self.insert(SourceEntry {
            id,
            path,
            revision,
            rope: Rope::from_str(&text),
            residency: SourceResidency::OpenEditable,
        });
    }

    pub fn apply_edits(
        &self,
        document_id: &PhysicalDocumentId,
        revision: RevisionId,
        edits: Vec<TextEdit>,
    ) -> Option<()> {
        let mut entry = self.inner.entries.get_mut(document_id)?;
        apply_text_edits(&mut entry.rope, edits);
        entry.revision = revision;
        entry.residency = SourceResidency::OpenEditable;
        Some(())
    }

    pub fn close_editable(&self, document_id: &PhysicalDocumentId) -> Option<SourceEntry> {
        let mut entry = self.inner.entries.get_mut(document_id)?;
        if entry.residency != SourceResidency::OpenEditable {
            return Some(entry.clone());
        }
        if let Some(path) = entry.path.clone()
            && let Ok(file) = File::open(&path)
            && let Ok(rope) = Rope::from_reader(file)
        {
            entry.revision = file_revision(&path).unwrap_or_default();
            entry.rope = rope;
            entry.residency = SourceResidency::AnalysisSnapshot;
            return Some(entry.clone());
        }
        Some(entry.clone())
    }

    pub fn region_rope(&self, region: &ProjectSourceRegion) -> Option<(RevisionId, Rope)> {
        let entry = self.inner.entries.get(&region.physical_document)?;
        let range = region.content_range.as_usize();
        if range.end > entry.rope.len() {
            return None;
        }
        Some((entry.revision, Rope::from(entry.rope.slice(range))))
    }
}

impl Default for ProjectAnalysis {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectAnalysis {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ProjectAnalysisInner {
                sources: ProjectSourceDb::new(),
                analysis: AnalysisHost::new(),
                contexts: DashMap::new(),
                source_files: DashMap::new(),
                source_scopes: DashMap::new(),
                regions_by_scope: DashMap::new(),
                effective_contexts: DashMap::new(),
            }),
        }
    }

    /// Returns the shared project source DB handle.
    pub fn sources(&self) -> ProjectSourceDb {
        self.inner.sources.clone()
    }

    /// Inserts or replaces a physical source document in the project source DB.
    pub fn insert_source(&self, entry: SourceEntry) {
        self.inner.sources.insert(entry);
    }

    /// Returns physical document metadata without cloning retained source text.
    pub fn document(&self, document_id: &PhysicalDocumentId) -> Option<PhysicalDocument> {
        self.inner.sources.physical_document(document_id)
    }

    /// Publishes a context's visible source regions to the shared analysis host.
    ///
    /// Each unique source region is inserted into the analysis host once, then
    /// referenced by every context source set that includes it.
    pub fn insert_bundle(&self, bundle: DocumentBundle) {
        let context_id = bundle.context.id.clone();
        let mut sources = Vec::new();

        for region in &bundle.regions {
            let Some(file) = self.ensure_source_file(region) else {
                continue;
            };
            sources.push(ProjectContextSource {
                context: context_id.clone(),
                physical_document: region.physical_document.clone(),
                file,
                content_range: region.content_range,
                source_offset: region.source_offset,
                resolution_scope: self
                    .inner
                    .source_scopes
                    .get(region)
                    .map(|scope| scope.clone())
                    .unwrap_or_else(|| bundle.context.label.clone()),
            });
        }

        self.inner.analysis.set_context_files(
            &context_id,
            sources.iter().map(|source| source.file).collect(),
        );
        self.inner.contexts.insert(
            context_id,
            Arc::new(AnalysisContextState { bundle, sources }),
        );
    }

    /// Returns the shared Picante-backed analysis host.
    pub fn analysis_host(&self) -> AnalysisHost {
        self.inner.analysis.clone()
    }

    /// Returns the context label and identity for an effective context.
    pub fn context(&self, context_id: &AnalysisContextId) -> Option<AnalysisContext> {
        self.inner
            .contexts
            .get(context_id)
            .map(|state| state.bundle.context.clone())
    }

    /// Returns every effective context that includes a physical document.
    pub fn contexts_for_document(&self, document_id: &PhysicalDocumentId) -> Vec<AnalysisContext> {
        let mut contexts = self
            .inner
            .contexts
            .iter()
            .filter_map(|entry| {
                entry
                    .sources
                    .iter()
                    .any(|source| &source.physical_document == document_id)
                    .then(|| entry.bundle.context.clone())
            })
            .collect::<Vec<_>>();
        contexts.sort_by_key(|context| context.id.clone());
        contexts
    }

    /// Returns the number of effective analysis contexts.
    pub fn context_count(&self) -> usize {
        self.inner.contexts.len()
    }

    /// Returns context source mappings for every context containing a document.
    pub fn context_sources_for_document(
        &self,
        document_id: &PhysicalDocumentId,
    ) -> Vec<ProjectContextSource> {
        let mut sources = self
            .inner
            .contexts
            .iter()
            .flat_map(|entry| entry.sources.clone())
            .filter(|source| &source.physical_document == document_id)
            .collect::<Vec<_>>();
        sources.sort_by_key(|source| {
            (
                source.context.clone(),
                source.content_range.start,
                source.content_range.end,
            )
        });
        sources
    }

    /// Returns effective analysis contexts in deterministic order.
    pub fn contexts(&self) -> Vec<AnalysisContext> {
        let mut contexts = self
            .inner
            .contexts
            .iter()
            .map(|entry| entry.bundle.context.clone())
            .collect::<Vec<_>>();
        contexts.sort_by_key(|context| context.id.clone());
        contexts
    }

    pub fn open_documents(&self) -> Vec<PhysicalDocumentId> {
        self.inner
            .sources
            .documents_with_residency(SourceResidency::OpenEditable)
    }

    /// Returns source mappings for one effective context.
    pub fn context_sources(&self, context_id: &AnalysisContextId) -> Vec<ProjectContextSource> {
        let mut sources = self
            .inner
            .contexts
            .get(context_id)
            .map(|state| state.sources.clone())
            .unwrap_or_default();
        sources.sort_by_key(|source| {
            (
                source.resolution_scope.clone(),
                source.physical_document.clone(),
                source.content_range.start,
                source.content_range.end,
            )
        });
        sources
    }

    /// Returns source-scope ownership records for project documents.
    pub fn source_scopes(&self) -> Vec<ProjectSourceScope> {
        let mut scopes = self
            .inner
            .source_scopes
            .iter()
            .filter_map(|entry| {
                let source = self.inner.sources.source(&entry.key().physical_document)?;
                Some(ProjectSourceScope {
                    physical_document: entry.key().physical_document.clone(),
                    path: source.path,
                    source_offset: entry.key().source_offset,
                    resolution_scope: entry.value().clone(),
                })
            })
            .collect::<Vec<_>>();
        scopes.sort_by_key(|scope| {
            (
                scope.resolution_scope.clone(),
                scope.path.clone(),
                scope.source_offset,
            )
        });
        scopes
    }

    fn selected_source_at_byte(
        &self,
        document_id: &PhysicalDocumentId,
        byte: usize,
    ) -> Option<(ProjectContextSource, usize)> {
        let mut candidates = self
            .context_sources_for_document(document_id)
            .into_iter()
            .filter(|source| {
                byte >= source.content_range.start as usize
                    && byte <= source.content_range.end as usize
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|source| {
            (
                source.context.clone(),
                source.content_range.start,
                source.content_range.end,
            )
        });
        let source = candidates.into_iter().next()?;
        let local_byte = byte.saturating_sub(source.content_range.start as usize);
        Some((source, local_byte))
    }

    pub fn open_document(
        &self,
        document_id: PhysicalDocumentId,
        path: Option<PathBuf>,
        version: i32,
        text: String,
    ) {
        self.inner.sources.open_editable(
            document_id.clone(),
            path,
            revision_from_version(version),
            text,
        );
        self.refresh_document_regions(&document_id);
    }

    pub fn change_document(
        &self,
        document_id: &PhysicalDocumentId,
        version: i32,
        edits: Vec<TextEdit>,
    ) -> Option<()> {
        self.inner
            .sources
            .apply_edits(document_id, revision_from_version(version), edits)?;
        self.refresh_document_regions(document_id);
        Some(())
    }

    pub fn replace_document(
        &self,
        document_id: &PhysicalDocumentId,
        version: i32,
        text: String,
    ) -> Option<()> {
        let source = self.inner.sources.source(document_id)?;
        self.open_document(document_id.clone(), source.path, version, text);
        Some(())
    }

    pub fn close_document(&self, document_id: &PhysicalDocumentId) -> Option<SourceEntry> {
        let source = self.inner.sources.close_editable(document_id)?;
        self.refresh_document_regions(document_id);
        Some(source)
    }

    pub fn document_snapshot(&self, document_id: &PhysicalDocumentId) -> Option<DocumentSnapshot> {
        let source = self.inner.sources.source(document_id)?;
        let file = self
            .context_sources_for_document(document_id)
            .into_iter()
            .next()
            .map_or(FileId(u32::MAX), |source| source.file);
        Some(DocumentSnapshot {
            file,
            uri: source
                .path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| source.id.0.display().to_string()),
            version: source.revision.0.min(i32::MAX as u64) as i32,
            revision: source.revision,
            rope: source.rope,
        })
    }

    pub fn document_byte_offset(
        &self,
        document_id: &PhysicalDocumentId,
        position: TextPosition,
    ) -> Option<usize> {
        let source = self.inner.sources.source(document_id)?;
        Some(position_to_byte(&source.rope, position))
    }

    pub async fn completions(
        &self,
        document_id: &PhysicalDocumentId,
        position: TextPosition,
    ) -> Option<Vec<CompletionItem>> {
        let byte = self.document_byte_offset(document_id, position)?;
        let Some((source, local_byte)) = self.selected_source_at_byte(document_id, byte) else {
            return Some(Vec::new());
        };
        let analysis = self.inner.analysis.analyze(source.file).await?;
        let catalog = self.inner.analysis.catalog();
        let scope = self
            .inner
            .analysis
            .completion_scope_in_context(&source.context, source.file)
            .await
            .unwrap_or_default();
        Some(completions_at(
            &analysis.parse,
            &catalog,
            local_byte,
            &scope,
        ))
    }

    pub async fn completion_context_debug(
        &self,
        document_id: &PhysicalDocumentId,
        position: TextPosition,
    ) -> Option<String> {
        let byte = self.document_byte_offset(document_id, position)?;
        let (source, local_byte) = self.selected_source_at_byte(document_id, byte)?;
        let analysis = self.inner.analysis.analyze(source.file).await?;
        let catalog = self.inner.analysis.catalog();
        Some(crate::host::format_completion_context(
            &cursor_context(&analysis.parse, &catalog, local_byte),
            &catalog,
        ))
    }

    pub async fn hover(
        &self,
        document_id: &PhysicalDocumentId,
        position: TextPosition,
    ) -> Option<HoverInfo> {
        let byte = self.document_byte_offset(document_id, position)?;
        let (source, local_byte) = self.selected_source_at_byte(document_id, byte)?;
        let analysis = self.inner.analysis.analyze(source.file).await?;
        let catalog = self.inner.analysis.catalog();
        hover_at(&analysis.parse.source_file, &catalog, local_byte)
    }

    pub async fn definition(
        &self,
        document_id: &PhysicalDocumentId,
        position: TextPosition,
    ) -> Option<DefinitionResult> {
        let byte = self.document_byte_offset(document_id, position)?;
        let (source, local_byte) = self.selected_source_at_byte(document_id, byte)?;
        let analysis = self.inner.analysis.analyze(source.file).await?;
        let catalog = self.inner.analysis.catalog();
        match definition_target_at(&analysis.parse.source_file, &catalog, local_byte)? {
            DefinitionTarget::Catalog(target) => Some(DefinitionResult::Catalog(target)),
            DefinitionTarget::Fragment { name } => self
                .find_fragment_definition_in_context(&source.context, &name)
                .await
                .map(DefinitionResult::Source),
        }
    }

    pub async fn document_format(
        &self,
        document_id: &PhysicalDocumentId,
    ) -> Option<DocumentFormat> {
        let snapshot = self.document_snapshot(document_id)?;
        let mut sources = self.context_sources_for_document(document_id);
        sources.sort_by_key(|source| (source.content_range.start, source.content_range.end));
        sources.dedup_by_key(|source| (source.content_range.start, source.content_range.end));
        let Some(source) = sources.first() else {
            return None;
        };
        if sources.len() == 1
            && source.content_range.start == 0
            && source.content_range.end as usize == snapshot.rope.len()
        {
            let formatted = self.inner.analysis.format(source.file).await?;
            return Some(DocumentFormat {
                snapshot,
                formatted,
            });
        }

        let mut text = snapshot.rope.to_string();
        let mut diagnostics = Vec::new();
        let mut replacements = Vec::with_capacity(sources.len());
        for source in sources {
            let formatted = self.inner.analysis.format(source.file).await?;
            diagnostics.extend(formatted.diagnostics.clone());
            replacements.push((
                source.content_range.start as usize,
                source.content_range.end as usize,
                formatted.text,
            ));
        }
        if !diagnostics.is_empty() {
            return Some(DocumentFormat {
                snapshot,
                formatted: FormattedText {
                    text,
                    confidence: FormatConfidence::PreserveOriginal,
                    diagnostics,
                },
            });
        }

        replacements.sort_by_key(|(start, _, _)| *start);
        for (start, end, replacement) in replacements.into_iter().rev() {
            let original = &text[start..end];
            let replacement = format_embedded_replacement(original, replacement);
            text.replace_range(start..end, &replacement);
        }

        Some(DocumentFormat {
            snapshot,
            formatted: FormattedText {
                text,
                confidence: FormatConfidence::Full,
                diagnostics,
            },
        })
    }

    pub async fn semantic_tokens(
        &self,
        document_id: &PhysicalDocumentId,
    ) -> Option<DocumentSemanticTokens> {
        let snapshot = self.document_snapshot(document_id)?;
        let sources = self.context_sources_for_document(document_id);
        if sources.len() != 1 {
            return None;
        }
        let source = sources.into_iter().next()?;
        if source.content_range.start != 0
            || source.content_range.end as usize != snapshot.rope.len()
        {
            return None;
        }
        let analysis = self.inner.analysis.analyze(source.file).await?;
        let catalog = self.inner.analysis.catalog();
        Some(DocumentSemanticTokens {
            snapshot,
            tokens: semantic_tokens_at(&analysis.parse, &catalog),
        })
    }

    pub async fn generation_model(&self) -> ProjectGenerationModel {
        let contexts = self.generation_contexts().await;
        let query_count = contexts
            .iter()
            .flat_map(|context| {
                context.definitions.iter().filter(|definition| {
                    matches!(&definition.definition, DefinitionRecord::Query(_))
                        && definition.resolution_scope == context.context.label
                })
            })
            .count();

        let mut diagnostics = Vec::new();
        let mut seen_documents = HashSet::<PhysicalDocumentId>::new();
        for scope in self.source_scopes() {
            if seen_documents.insert(scope.physical_document.clone()) {
                diagnostics.extend(
                    self.analysis_diagnostics_for_document(&scope.physical_document)
                        .await,
                );
            }
        }
        diagnostics.extend(self.project_validation_diagnostics_for_contexts(&contexts));
        sort_presented_diagnostics(&mut diagnostics);

        let source_scopes = self.source_scopes();
        ProjectGenerationModel {
            document_count: source_scopes.len(),
            query_count,
            contexts,
            diagnostics,
            source_scopes,
        }
    }

    pub fn load_from(start_dir: &Path) -> dsql_project::Result<Self> {
        let project = dsql_project::Project::load_from(start_dir)?;
        Self::load_from_project(&project)
    }

    pub fn load_from_project(project: &dsql_project::Project) -> dsql_project::Result<Self> {
        let analysis = Self::new();
        analysis.reload_from_project(project)?;
        Ok(analysis)
    }

    pub fn reload_from_project(&self, project: &dsql_project::Project) -> dsql_project::Result<()> {
        let effective_contexts = effective_resolution_contexts(project)?;
        let catalog = project.load_catalog()?;
        let lint_options = project.lint_options();
        let project_documents = dsql_project::load_project_documents(project)?;
        let mut regions_by_scope = BTreeMap::<String, Vec<ProjectSourceRegion>>::new();

        let old_files = self
            .inner
            .source_files
            .iter()
            .map(|entry| *entry.value())
            .collect::<Vec<_>>();
        for file in old_files {
            self.inner.analysis.remove_file(file);
        }
        self.inner.contexts.clear();
        self.inner.source_files.clear();
        self.inner.source_scopes.clear();
        self.inner.regions_by_scope.clear();
        self.inner.effective_contexts.clear();
        for context in &effective_contexts {
            self.inner
                .effective_contexts
                .insert(context.name.clone(), context.clone());
        }

        for project_document in project_documents {
            let document_id = self
                .inner
                .sources
                .load_analysis_snapshot(&project_document.path)?;
            let start = project_document.source_offset;
            let end = start + project_document.text.len();
            regions_by_scope
                .entry(project_document.resolution_scope.clone())
                .or_default()
                .push({
                    let region = ProjectSourceRegion {
                        physical_document: document_id,
                        content_range: TextRange::new(start, end),
                        source_offset: start as u32,
                    };
                    self.inner
                        .source_scopes
                        .insert(region.clone(), project_document.resolution_scope.clone());
                    region
                });
        }

        for (scope, regions) in &regions_by_scope {
            self.inner
                .regions_by_scope
                .insert(scope.clone(), regions.clone());
        }

        for effective in effective_contexts {
            let mut seen = HashSet::<ProjectSourceRegion>::new();
            let mut regions = Vec::new();
            for scope in &effective.scopes {
                if let Some(scope_regions) = regions_by_scope.get(scope) {
                    for region in scope_regions {
                        if seen.insert(region.clone()) {
                            regions.push(region.clone());
                        }
                    }
                }
            }
            let context = AnalysisContext {
                id: AnalysisContextId(effective.name.clone()),
                label: effective.name,
            };
            self.insert_bundle(DocumentBundle { context, regions });
        }
        self.inner.analysis.set_catalog(catalog);
        self.inner.analysis.set_lint_options(lint_options);

        Ok(())
    }

    pub fn present_diagnostic(&self, diagnostic: ProjectDiagnostic) -> Option<PresentedDiagnostic> {
        let source = self.inner.sources.source(&diagnostic.physical_document)?;
        let physical_range = TextRange::new(
            diagnostic.source_offset as usize + diagnostic.diagnostic.range.start as usize,
            diagnostic.source_offset as usize + diagnostic.diagnostic.range.end as usize,
        );
        let context = if self.inner.contexts.len() > 1 {
            Some(self.context(&diagnostic.context)?)
        } else {
            None
        };
        Some(PresentedDiagnostic {
            context,
            physical_document: diagnostic.physical_document,
            path: source.path,
            source_offset: diagnostic.source_offset,
            embedded_range: diagnostic.diagnostic.range,
            range: physical_range,
            start_position: byte_to_position(&source.rope, physical_range.start as usize),
            end_position: byte_to_position(&source.rope, physical_range.end as usize),
            diagnostic: Diagnostic {
                range: physical_range,
                ..diagnostic.diagnostic
            },
        })
    }

    pub async fn diagnostics_for_document(
        &self,
        document_id: &PhysicalDocumentId,
    ) -> Vec<PresentedDiagnostic> {
        let mut diagnostics = self.analysis_diagnostics_for_document(document_id).await;
        diagnostics.extend(
            self.project_validation_diagnostics_for_document(document_id)
                .await,
        );
        sort_presented_diagnostics(&mut diagnostics);
        diagnostics
    }

    async fn analysis_diagnostics_for_document(
        &self,
        document_id: &PhysicalDocumentId,
    ) -> Vec<PresentedDiagnostic> {
        let mut diagnostics = Vec::new();
        for source in self.context_sources_for_document(document_id) {
            let Some(source_diagnostics) = self
                .inner
                .analysis
                .diagnostics_in_context(&source.context, source.file)
                .await
            else {
                continue;
            };
            diagnostics.extend(source_diagnostics.into_iter().filter_map(|diagnostic| {
                self.present_diagnostic(ProjectDiagnostic {
                    context: source.context.clone(),
                    physical_document: source.physical_document.clone(),
                    source_offset: source.source_offset,
                    diagnostic,
                })
            }));
        }
        diagnostics
    }

    async fn generation_contexts(&self) -> Vec<ProjectGenerationContext> {
        let mut contexts = Vec::new();
        for context in self.contexts() {
            let mut definitions = Vec::new();
            let definitions_by_file = self
                .inner
                .analysis
                .context_definitions(&context.id)
                .await
                .map(|definitions| definitions.files)
                .unwrap_or_default();
            for definition_file in definitions_by_file {
                let Some(source) = self
                    .context_sources(&context.id)
                    .into_iter()
                    .find(|source| source.file == definition_file.file)
                else {
                    continue;
                };
                let path = self
                    .inner
                    .sources
                    .source(&source.physical_document)
                    .and_then(|entry| entry.path);
                for definition in definition_file.definitions {
                    definitions.push(ProjectGenerationDefinition {
                        physical_document: source.physical_document.clone(),
                        path: path.clone(),
                        file: source.file,
                        source_offset: source.source_offset,
                        resolution_scope: source.resolution_scope.clone(),
                        definition,
                    });
                }
            }
            definitions.sort_by_key(|definition| {
                (
                    definition.resolution_scope.clone(),
                    definition.path.clone(),
                    definition.source_offset,
                    definition.file.0,
                )
            });
            contexts.push(ProjectGenerationContext {
                context,
                definitions,
            });
        }
        contexts
    }

    async fn project_validation_diagnostics_for_document(
        &self,
        document_id: &PhysicalDocumentId,
    ) -> Vec<PresentedDiagnostic> {
        let contexts = self.generation_contexts().await;
        self.project_validation_diagnostics_for_contexts(&contexts)
            .into_iter()
            .filter(|diagnostic| &diagnostic.physical_document == document_id)
            .collect()
    }

    fn project_validation_diagnostics_for_contexts(
        &self,
        contexts: &[ProjectGenerationContext],
    ) -> Vec<PresentedDiagnostic> {
        let catalog = self.inner.analysis.catalog();
        let mut diagnostics = Vec::new();
        for context in contexts {
            let mut fragments = FragmentMap::default();
            for definition in &context.definitions {
                if let DefinitionRecord::Fragment(fragment) = &definition.definition {
                    fragments.insert(fragment.clone());
                }
            }

            let mut seen_queries = HashMap::<&str, &ProjectGenerationDefinition>::new();
            for definition in &context.definitions {
                match &definition.definition {
                    DefinitionRecord::Query(query)
                        if definition.resolution_scope == context.context.label =>
                    {
                        if let Some(query_name) = query.key.name.as_deref() {
                            if seen_queries.insert(query_name, definition).is_some()
                                && let Some(diagnostic) = self.project_validation_diagnostic(
                                    &context.context.id,
                                    &definition.physical_document,
                                    definition.source_offset,
                                    query.name_range.unwrap_or(query.range),
                                    DiagnosticCode::DuplicateDefinition,
                                    format!(
                                        "duplicate query `{}` in resolution map `{}`",
                                        query_name, context.context.label
                                    ),
                                )
                            {
                                diagnostics.push(diagnostic);
                            }

                            let variables =
                                infer_query_variable_bindings(query, &fragments, &catalog).bindings;
                            if let Some(diagnostic) = self.duplicate_anonymous_variable_diagnostic(
                                &context.context.id,
                                "query",
                                query_name,
                                &definition.physical_document,
                                definition.source_offset,
                                &variables,
                            ) {
                                diagnostics.push(diagnostic);
                            }
                        } else if let Some(diagnostic) = self.project_validation_diagnostic(
                            &context.context.id,
                            &definition.physical_document,
                            definition.source_offset,
                            query.name_range.unwrap_or(query.range),
                            DiagnosticCode::AnonymousQuery,
                            "anonymous queries cannot be generated",
                        ) {
                            diagnostics.push(diagnostic);
                        }
                    }
                    DefinitionRecord::Fragment(fragment) => {
                        let variables =
                            infer_fragment_variable_bindings(fragment, &fragments, &catalog)
                                .bindings;
                        if let Some(diagnostic) = self.duplicate_anonymous_variable_diagnostic(
                            &context.context.id,
                            "fragment",
                            &fragment.key.name,
                            &definition.physical_document,
                            definition.source_offset,
                            &variables,
                        ) {
                            diagnostics.push(diagnostic);
                        }
                    }
                    DefinitionRecord::Query(_) => {}
                }
            }
        }
        sort_presented_diagnostics(&mut diagnostics);
        diagnostics
    }

    fn duplicate_anonymous_variable_diagnostic(
        &self,
        context: &AnalysisContextId,
        definition_kind: &str,
        definition_name: &str,
        physical_document: &PhysicalDocumentId,
        source_offset: u32,
        variables: &[VariableBinding],
    ) -> Option<PresentedDiagnostic> {
        let mut anonymous_paths = HashMap::<&str, &VariableBinding>::new();
        for binding in variables.iter().filter(|binding| binding.name.is_none()) {
            if let Some(previous) = anonymous_paths.insert(&binding.path, binding) {
                return self.project_validation_diagnostic(
                    context,
                    physical_document,
                    source_offset,
                    binding.range,
                    DiagnosticCode::DuplicateAnonymousVariable,
                    format!(
                        "{definition_kind} `{definition_name}` has multiple anonymous variables for `{}`; name one of them to disambiguate",
                        previous.path
                    ),
                );
            }
        }
        None
    }

    fn project_validation_diagnostic(
        &self,
        context: &AnalysisContextId,
        physical_document: &PhysicalDocumentId,
        source_offset: u32,
        range: TextRange,
        code: DiagnosticCode,
        message: impl Into<String>,
    ) -> Option<PresentedDiagnostic> {
        self.present_diagnostic(ProjectDiagnostic {
            context: context.clone(),
            physical_document: physical_document.clone(),
            source_offset,
            diagnostic: Diagnostic {
                range,
                severity: Severity::Error,
                code,
                source: DiagnosticSource::Generate,
                message: message.into(),
            },
        })
    }

    pub async fn diagnostics_for_path(&self, path: &Path) -> Vec<PresentedDiagnostic> {
        self.diagnostics_for_document(&PhysicalDocumentId(path.to_path_buf()))
            .await
    }

    async fn find_fragment_definition_in_context(
        &self,
        context: &AnalysisContextId,
        name: &str,
    ) -> Option<SourceDefinition> {
        let definitions = self.inner.analysis.context_definitions(context).await?;
        for definition_file in definitions.files {
            for definition in definition_file.definitions {
                let DefinitionRecord::Fragment(fragment) = definition else {
                    continue;
                };
                if fragment.key.name != name {
                    continue;
                }
                let source = self
                    .context_sources(context)
                    .into_iter()
                    .find(|source| source.file == definition_file.file)?;
                let range = TextRange::new(
                    source.content_range.start as usize + fragment.name_range.start as usize,
                    source.content_range.start as usize + fragment.name_range.end as usize,
                );
                return Some(SourceDefinition {
                    uri: source.physical_document.0.to_string_lossy().to_string(),
                    range,
                    kind: SourceDefinitionKind::Fragment,
                });
            }
        }
        None
    }

    fn ensure_source_file(&self, region: &ProjectSourceRegion) -> Option<FileId> {
        let (revision, rope) = self.inner.sources.region_rope(region)?;
        if let Some(file) = self.inner.source_files.get(region).map(|file| *file) {
            self.inner
                .analysis
                .set_file_source(file, revision, SourceSnapshot::from_rope(rope));
            return Some(file);
        }
        let file = self
            .inner
            .analysis
            .create_file_with_revision(revision, SourceSnapshot::from_rope(rope));
        self.inner.source_files.insert(region.clone(), file);
        Some(file)
    }

    fn refresh_document_regions(&self, document_id: &PhysicalDocumentId) {
        let mut scopes = BTreeSet::<String>::new();
        let mut old_regions = Vec::<ProjectSourceRegion>::new();

        for mut entry in self.inner.regions_by_scope.iter_mut() {
            let scope = entry.key().clone();
            entry.value_mut().retain(|region| {
                if &region.physical_document == document_id {
                    scopes.insert(scope.clone());
                    old_regions.push(region.clone());
                    false
                } else {
                    true
                }
            });
        }

        for region in old_regions {
            self.inner.source_scopes.remove(&region);
            if let Some((_, file)) = self.inner.source_files.remove(&region) {
                self.inner.analysis.remove_file(file);
            }
        }

        if scopes.is_empty() {
            scopes.insert(dsql_project::DEFAULT_RESOLUTION_SCOPE.to_string());
        }

        if let Some(source) = self.inner.sources.source(document_id) {
            let regions = source_regions_for_entry(&source);
            for scope in &scopes {
                let mut entry = self
                    .inner
                    .regions_by_scope
                    .entry(scope.clone())
                    .or_default();
                for region in &regions {
                    self.inner
                        .source_scopes
                        .insert(region.clone(), scope.clone());
                    entry.push(region.clone());
                }
            }
        }

        self.rebuild_contexts_from_scopes();
    }

    fn rebuild_contexts_from_scopes(&self) {
        self.inner.contexts.clear();
        let mut effective_contexts = self
            .inner
            .effective_contexts
            .iter()
            .map(|entry| entry.value().clone())
            .collect::<Vec<_>>();
        if effective_contexts.is_empty() {
            effective_contexts = default_effective_contexts();
        }
        effective_contexts.sort_by(|left, right| left.name.cmp(&right.name));

        for effective in effective_contexts {
            let mut seen = HashSet::<ProjectSourceRegion>::new();
            let mut regions = Vec::new();
            for scope in &effective.scopes {
                if let Some(scope_regions) = self.inner.regions_by_scope.get(scope) {
                    for region in scope_regions.iter() {
                        if seen.insert(region.clone()) {
                            regions.push(region.clone());
                        }
                    }
                }
            }
            let context = AnalysisContext {
                id: AnalysisContextId(effective.name.clone()),
                label: effective.name,
            };
            self.insert_bundle(DocumentBundle { context, regions });
        }
    }
}

fn effective_resolution_contexts(
    project: &dsql_project::Project,
) -> dsql_project::Result<Vec<EffectiveResolutionContext>> {
    if project.config.resolution.is_empty() {
        return Ok(vec![EffectiveResolutionContext {
            name: dsql_project::DEFAULT_RESOLUTION_SCOPE.to_string(),
            scopes: vec![dsql_project::DEFAULT_RESOLUTION_SCOPE.to_string()],
        }]);
    }

    validate_resolution_imports(project)?;
    let imported = project
        .config
        .resolution
        .values()
        .flat_map(|config| config.imports.iter().cloned())
        .collect::<BTreeSet<_>>();
    let mut effective = Vec::new();
    for name in project.config.resolution.keys() {
        if imported.contains(name) {
            continue;
        }
        let mut scopes = Vec::new();
        let mut seen = BTreeSet::new();
        collect_resolution_scope_closure(project, name, &mut seen, &mut scopes);
        effective.push(EffectiveResolutionContext {
            name: name.clone(),
            scopes,
        });
    }
    Ok(effective)
}

fn validate_resolution_imports(project: &dsql_project::Project) -> dsql_project::Result<()> {
    let mut states = HashMap::<String, VisitState>::new();
    let mut stack = Vec::<String>::new();
    for name in project.config.resolution.keys() {
        visit_resolution_scope(project, name, &mut states, &mut stack)?;
    }
    Ok(())
}

fn visit_resolution_scope(
    project: &dsql_project::Project,
    name: &str,
    states: &mut HashMap<String, VisitState>,
    stack: &mut Vec<String>,
) -> dsql_project::Result<()> {
    match states.get(name) {
        Some(VisitState::Visited) => return Ok(()),
        Some(VisitState::Visiting) => {
            let cycle_start = stack
                .iter()
                .position(|scope| scope == name)
                .unwrap_or_default();
            let mut cycle = stack[cycle_start..].to_vec();
            cycle.push(name.to_string());
            return Err(dsql_project::ProjectError::CyclicResolutionImport { cycle });
        }
        None => {}
    }

    let Some(config) = project.config.resolution.get(name) else {
        return Ok(());
    };
    states.insert(name.to_string(), VisitState::Visiting);
    stack.push(name.to_string());
    for import in &config.imports {
        if !project.config.resolution.contains_key(import) {
            return Err(dsql_project::ProjectError::UnknownResolutionImport {
                scope: name.to_string(),
                import: import.clone(),
            });
        }
        visit_resolution_scope(project, import, states, stack)?;
    }
    stack.pop();
    states.insert(name.to_string(), VisitState::Visited);
    Ok(())
}

fn collect_resolution_scope_closure(
    project: &dsql_project::Project,
    name: &str,
    seen: &mut BTreeSet<String>,
    scopes: &mut Vec<String>,
) {
    if !seen.insert(name.to_string()) {
        return;
    }
    scopes.push(name.to_string());
    if let Some(config) = project.config.resolution.get(name) {
        for import in &config.imports {
            collect_resolution_scope_closure(project, import, seen, scopes);
        }
    }
}

fn default_effective_contexts() -> Vec<EffectiveResolutionContext> {
    vec![EffectiveResolutionContext {
        name: dsql_project::DEFAULT_RESOLUTION_SCOPE.to_string(),
        scopes: vec![dsql_project::DEFAULT_RESOLUTION_SCOPE.to_string()],
    }]
}

fn source_regions_for_entry(source: &SourceEntry) -> Vec<ProjectSourceRegion> {
    let source_path = source.path.as_deref().unwrap_or(source.id.0.as_path());
    if source_path
        .extension()
        .and_then(|extension| extension.to_str())
        == Some("dsql")
    {
        return vec![ProjectSourceRegion {
            physical_document: source.id.clone(),
            content_range: TextRange::new(0, source.rope.len()),
            source_offset: 0,
        }];
    }

    let text = source.rope.to_string();
    let embedding = RegexEmbedding::new(default_typescript_regex_pattern());
    embedding
        .extract(&text)
        .unwrap_or_default()
        .into_iter()
        .map(|region| ProjectSourceRegion {
            physical_document: source.id.clone(),
            content_range: region.content_range,
            source_offset: region.content_range.start,
        })
        .collect()
}

fn revision_from_version(version: i32) -> RevisionId {
    RevisionId(version.max(0) as u64)
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

fn sort_presented_diagnostics(diagnostics: &mut [PresentedDiagnostic]) {
    diagnostics.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.range.start.cmp(&right.range.start))
            .then(left.range.end.cmp(&right.range.end))
            .then(
                left.context
                    .as_ref()
                    .map(|context| context.id.clone())
                    .cmp(&right.context.as_ref().map(|context| context.id.clone())),
            )
            .then(left.diagnostic.message.cmp(&right.diagnostic.message))
    });
}

fn file_revision(path: &Path) -> io::Result<RevisionId> {
    Ok(RevisionId(
        fs::metadata(path)?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs()),
    ))
}

fn byte_to_position(rope: &Rope, byte: usize) -> SourcePosition {
    let byte = byte.min(rope.len());
    let line = rope.byte_to_line_idx(byte, LineType::LF_CR);
    let line_start = rope.line_to_byte_idx(line, LineType::LF_CR);
    let character = rope.slice(line_start..byte).len_utf16();
    SourcePosition {
        line: line as u32,
        character: character as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsql_core::{DiagnosticCode, DiagnosticSource, Severity};
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn project_loading_builds_hosts_for_effective_contexts() {
        let root = temp_project_root("effective-contexts");
        fs::create_dir_all(root.join("dsql/schema")).unwrap();
        fs::create_dir_all(root.join("queries/shared")).unwrap();
        fs::create_dir_all(root.join("queries/api")).unwrap();
        fs::create_dir_all(root.join("queries/frontend")).unwrap();
        fs::write(
            root.join("dsql/dsql.toml"),
            r#"database_url = "<database url>"
default_schema = "app"
documents = []

[lint]
unindexed_scan_severity = "off"

[resolution.shared]
documents = ["queries/shared/**/*.dsql"]

[resolution.api]
documents = ["queries/api/**/*.dsql"]
imports = ["shared"]

[resolution.frontend]
documents = ["queries/frontend/**/*.dsql"]
imports = ["shared"]
"#,
        )
        .unwrap();
        fs::write(
            root.join("queries/shared/user_fields.dsql"),
            "fragment UserFields on users { id }\n",
        )
        .unwrap();
        fs::write(
            root.join("queries/api/users.dsql"),
            "query ApiUsers { users { id } }\n",
        )
        .unwrap();
        fs::write(
            root.join("queries/frontend/users.dsql"),
            "query FrontendUsers { users { id } }\n",
        )
        .unwrap();

        let project = dsql_project::Project::load_from(&root).unwrap();
        let analysis = ProjectAnalysis::load_from_project(&project).unwrap();
        let shared_id = PhysicalDocumentId(root.join("queries/shared/user_fields.dsql"));
        let shared_contexts = analysis
            .context_sources_for_document(&shared_id)
            .into_iter()
            .map(|source| source.context.0)
            .collect::<Vec<_>>();
        let shared_files = analysis
            .context_sources_for_document(&shared_id)
            .into_iter()
            .map(|source| source.file)
            .collect::<Vec<_>>();
        let api = analysis.analysis_host();

        assert_eq!(analysis.context_count(), 2);
        assert!(
            analysis
                .context(&AnalysisContextId("shared".to_string()))
                .is_none()
        );
        assert_eq!(shared_contexts, vec!["api", "frontend"]);
        assert_eq!(shared_files.len(), 2);
        assert_eq!(shared_files[0], shared_files[1]);
        assert_eq!(api.catalog().default_schema, "app");
        assert_eq!(api.lint_options().unindexed_scan_severity, None);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cloned_project_handles_share_source_and_context_state() {
        let analysis = ProjectAnalysis::new();
        let clone = analysis.clone();
        let document_id = PhysicalDocumentId(PathBuf::from("queries/users.dsql"));
        let source = "query Users { users { id } }\n";

        clone.insert_source(source_entry(document_id.clone(), source));
        analysis.insert_bundle(bundle("api", full_region(&document_id, source)));

        assert!(clone.document(&document_id).is_some());
        assert_eq!(
            clone
                .contexts_for_document(&document_id)
                .into_iter()
                .map(|context| context.id.0)
                .collect::<Vec<_>>(),
            vec!["api"]
        );
    }

    #[test]
    fn cloned_source_db_handles_share_source_state() {
        let sources = ProjectSourceDb::new();
        let clone = sources.clone();
        let document_id = PhysicalDocumentId(PathBuf::from("queries/shared.dsql"));

        sources.insert(source_entry(
            document_id.clone(),
            "fragment Shared on users { id }\n",
        ));

        assert_eq!(
            clone.physical_document(&document_id).unwrap().revision,
            RevisionId(1)
        );
    }

    #[test]
    fn context_diagnostics_do_not_merge_peer_context_definitions() {
        block_on(async {
            let analysis = ProjectAnalysis::new();
            let api_id = PhysicalDocumentId(PathBuf::from("queries/api/users.dsql"));
            let frontend_id = PhysicalDocumentId(PathBuf::from("queries/frontend/users.dsql"));
            let api_source = "query ApiUsers { users { ...FrontendOnly } }\n";
            let frontend_source = "fragment FrontendOnly on users { id }\n";

            analysis.insert_source(source_entry(api_id.clone(), api_source));
            analysis.insert_source(source_entry(frontend_id.clone(), frontend_source));
            analysis.insert_bundle(bundle("api", full_region(&api_id, api_source)));
            analysis.insert_bundle(bundle(
                "frontend",
                full_region(&frontend_id, frontend_source),
            ));

            let diagnostics = analysis.diagnostics_for_document(&api_id).await;

            assert!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.diagnostic.code == DiagnosticCode::UnknownFragment),
                "{diagnostics:?}"
            );
        });
    }

    #[test]
    fn project_loading_rejects_resolution_import_cycles() {
        let root = temp_project_root("resolution-cycle");
        fs::create_dir_all(root.join("dsql/schema")).unwrap();
        fs::write(
            root.join("dsql/dsql.toml"),
            r#"database_url = "<database url>"
documents = []

[resolution.api]
documents = []
imports = ["frontend"]

[resolution.frontend]
documents = []
imports = ["api"]
"#,
        )
        .unwrap();

        let project = dsql_project::Project::load_from(&root).unwrap();
        let error = match ProjectAnalysis::load_from_project(&project) {
            Ok(_) => panic!("expected cyclic resolution import error"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            dsql_project::ProjectError::CyclicResolutionImport { .. }
        ));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn presented_diagnostics_use_source_db_and_label_only_ambiguous_contexts() {
        let source = "prefix\nquery Users { users { id } }\n";
        let region_start = "prefix\n".len();
        let document_id = PhysicalDocumentId(PathBuf::from("src/users.ts"));
        let region = ProjectSourceRegion {
            physical_document: document_id.clone(),
            content_range: TextRange::new(region_start, source.len()),
            source_offset: region_start as u32,
        };
        let diagnostic = Diagnostic {
            range: TextRange::new(0, "query".len()),
            severity: Severity::Error,
            code: DiagnosticCode::TableNotFound,
            source: DiagnosticSource::Check,
            message: "missing table".to_string(),
        };

        let analysis = ProjectAnalysis::new();
        analysis.insert_source(SourceEntry {
            id: document_id.clone(),
            path: Some(PathBuf::from("src/users.ts")),
            revision: RevisionId(1),
            rope: Rope::from_str(source),
            residency: SourceResidency::OpenEditable,
        });
        analysis.insert_bundle(bundle("api", region.clone()));
        analysis.insert_bundle(bundle("frontend", region));

        let presented = analysis
            .present_diagnostic(ProjectDiagnostic {
                context: AnalysisContextId("api".to_string()),
                physical_document: document_id.clone(),
                source_offset: region_start as u32,
                diagnostic: diagnostic.clone(),
            })
            .unwrap();

        assert_eq!(presented.context.unwrap().label, "api");
        assert_eq!(
            presented.range,
            TextRange::new(region_start, region_start + "query".len())
        );
        assert_eq!(
            presented.start_position,
            SourcePosition {
                line: 1,
                character: 0
            }
        );

        let single_context = ProjectAnalysis::new();
        single_context.insert_source(SourceEntry {
            id: document_id.clone(),
            path: Some(PathBuf::from("src/users.ts")),
            revision: RevisionId(1),
            rope: Rope::from_str(source),
            residency: SourceResidency::OpenEditable,
        });
        single_context.insert_bundle(bundle(
            "api",
            ProjectSourceRegion {
                physical_document: document_id.clone(),
                content_range: TextRange::new(region_start, source.len()),
                source_offset: region_start as u32,
            },
        ));

        let presented = single_context
            .present_diagnostic(ProjectDiagnostic {
                context: AnalysisContextId("api".to_string()),
                physical_document: document_id,
                source_offset: region_start as u32,
                diagnostic,
            })
            .unwrap();

        assert!(presented.context.is_none());
    }

    fn bundle(name: &str, region: ProjectSourceRegion) -> DocumentBundle {
        DocumentBundle {
            context: AnalysisContext {
                id: AnalysisContextId(name.to_string()),
                label: name.to_string(),
            },
            regions: vec![region],
        }
    }

    fn full_region(document_id: &PhysicalDocumentId, source: &str) -> ProjectSourceRegion {
        ProjectSourceRegion {
            physical_document: document_id.clone(),
            content_range: TextRange::new(0, source.len()),
            source_offset: 0,
        }
    }

    fn source_entry(document_id: PhysicalDocumentId, source: &str) -> SourceEntry {
        SourceEntry {
            id: document_id.clone(),
            path: Some(document_id.0),
            revision: RevisionId(1),
            rope: Rope::from_str(source),
            residency: SourceResidency::AnalysisSnapshot,
        }
    }

    fn temp_project_root(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "dsql-frontend-{name}-{}-{unique}",
            std::process::id()
        ))
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);
        let mut future = Box::pin(future);
        loop {
            match Pin::new(&mut future).poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    fn noop_waker() -> Waker {
        fn clone(_: *const ()) -> RawWaker {
            raw_waker()
        }
        fn wake(_: *const ()) {}
        fn wake_by_ref(_: *const ()) {}
        fn drop(_: *const ()) {}
        fn raw_waker() -> RawWaker {
            RawWaker::new(
                std::ptr::null(),
                &RawWakerVTable::new(clone, wake, wake_by_ref, drop),
            )
        }
        unsafe { Waker::from_raw(raw_waker()) }
    }
}
