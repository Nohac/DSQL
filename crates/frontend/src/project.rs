use crate::{AnalysisContext, AnalysisContextId, AnalysisHost, FileId, RevisionId};
use dashmap::DashMap;
use dsql_core::{Diagnostic, SourceSnapshot, TextRange};
use ropey::{LineType, Rope};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs::{self, File},
    io,
    path::{Path, PathBuf},
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
pub struct ProjectSourceDb {
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

#[derive(Clone, Debug)]
pub struct ProjectHostSource {
    pub context: AnalysisContextId,
    pub physical_document: PhysicalDocumentId,
    pub file: FileId,
    pub content_range: TextRange,
    pub source_offset: u32,
}

pub struct ProjectAnalysis {
    sources: ProjectSourceDb,
    hosts: DashMap<AnalysisContextId, AnalysisHost>,
    bundles: DashMap<AnalysisContextId, DocumentBundle>,
    host_sources: DashMap<AnalysisContextId, Vec<ProjectHostSource>>,
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
    pub range: TextRange,
    pub start_position: SourcePosition,
    pub end_position: SourcePosition,
    pub diagnostic: Diagnostic,
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
            entries: DashMap::new(),
        }
    }

    pub fn insert(&self, entry: SourceEntry) {
        self.entries.insert(entry.id.clone(), entry);
    }

    pub fn load_analysis_snapshot(
        &self,
        path: impl AsRef<Path>,
    ) -> dsql_project::Result<PhysicalDocumentId> {
        let path = path.as_ref();
        let id = PhysicalDocumentId(path.to_path_buf());
        if self.entries.contains_key(&id) {
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
        self.entries.get(document_id).map(|entry| PhysicalDocument {
            id: entry.id.clone(),
            path: entry.path.clone(),
            revision: entry.revision,
            residency: entry.residency,
        })
    }

    pub fn source(&self, document_id: &PhysicalDocumentId) -> Option<SourceEntry> {
        self.entries.get(document_id).map(|entry| entry.clone())
    }

    pub fn region_rope(&self, region: &ProjectSourceRegion) -> Option<(RevisionId, Rope)> {
        let entry = self.entries.get(&region.physical_document)?;
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
            sources: ProjectSourceDb::new(),
            hosts: DashMap::new(),
            bundles: DashMap::new(),
            host_sources: DashMap::new(),
        }
    }

    pub fn sources(&self) -> &ProjectSourceDb {
        &self.sources
    }

    pub fn insert_source(&self, entry: SourceEntry) {
        self.sources.insert(entry);
    }

    pub fn document(&self, document_id: &PhysicalDocumentId) -> Option<PhysicalDocument> {
        self.sources.physical_document(document_id)
    }

    pub fn insert_bundle(&self, bundle: DocumentBundle) -> AnalysisHost {
        let context_id = bundle.context.id.clone();
        let host = AnalysisHost::with_context(bundle.context.clone());
        let mut sources = Vec::new();

        for region in &bundle.regions {
            let Some((revision, rope)) = self.sources.region_rope(region) else {
                continue;
            };
            let file = host.create_file_with_revision(revision, SourceSnapshot::from_rope(rope));
            sources.push(ProjectHostSource {
                context: context_id.clone(),
                physical_document: region.physical_document.clone(),
                file,
                content_range: region.content_range,
                source_offset: region.source_offset,
            });
        }

        self.bundles.insert(context_id.clone(), bundle);
        self.host_sources.insert(context_id.clone(), sources);
        self.hosts.insert(context_id, host.clone());
        host
    }

    pub fn host(&self, context_id: &AnalysisContextId) -> Option<AnalysisHost> {
        self.hosts.get(context_id).map(|host| host.clone())
    }

    pub fn hosts_for_document(&self, document_id: &PhysicalDocumentId) -> Vec<AnalysisHost> {
        let mut context_ids = self
            .bundles
            .iter()
            .filter_map(|entry| {
                entry
                    .regions
                    .iter()
                    .any(|region| &region.physical_document == document_id)
                    .then_some(entry.key().clone())
            })
            .collect::<Vec<_>>();
        context_ids.sort();
        context_ids.dedup();
        context_ids
            .into_iter()
            .filter_map(|context_id| self.host(&context_id))
            .collect()
    }

    pub fn host_count(&self) -> usize {
        self.hosts.len()
    }

    pub fn host_sources_for_document(
        &self,
        document_id: &PhysicalDocumentId,
    ) -> Vec<ProjectHostSource> {
        let mut sources = self
            .host_sources
            .iter()
            .flat_map(|entry| entry.value().clone())
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

    pub fn load_from(start_dir: &Path) -> dsql_project::Result<Self> {
        let project = dsql_project::Project::load_from(start_dir)?;
        Self::load_from_project(&project)
    }

    pub fn load_from_project(project: &dsql_project::Project) -> dsql_project::Result<Self> {
        let effective_contexts = effective_resolution_contexts(project)?;
        let catalog = project.load_catalog()?;
        let lint_options = project.lint_options();
        let project_documents = dsql_project::load_project_documents(project)?;
        let analysis = Self::new();
        let mut regions_by_scope = BTreeMap::<String, Vec<ProjectSourceRegion>>::new();

        for project_document in project_documents {
            let document_id = analysis
                .sources
                .load_analysis_snapshot(&project_document.path)?;
            let start = project_document.source_offset;
            let end = start + project_document.text.len();
            regions_by_scope
                .entry(project_document.resolution_scope.clone())
                .or_default()
                .push(ProjectSourceRegion {
                    physical_document: document_id,
                    content_range: TextRange::new(start, end),
                    source_offset: start as u32,
                });
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
            let host = analysis.insert_bundle(DocumentBundle { context, regions });
            host.set_catalog(catalog.clone());
            host.set_lint_options(lint_options);
        }

        Ok(analysis)
    }

    pub fn present_diagnostic(&self, diagnostic: ProjectDiagnostic) -> Option<PresentedDiagnostic> {
        let source = self.sources.source(&diagnostic.physical_document)?;
        let physical_range = TextRange::new(
            diagnostic.source_offset as usize + diagnostic.diagnostic.range.start as usize,
            diagnostic.source_offset as usize + diagnostic.diagnostic.range.end as usize,
        );
        let context = if self.hosts.len() > 1 {
            Some(self.host(&diagnostic.context)?.context())
        } else {
            None
        };
        Some(PresentedDiagnostic {
            context,
            physical_document: diagnostic.physical_document,
            path: source.path,
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
        let mut diagnostics = Vec::new();
        for source in self.host_sources_for_document(document_id) {
            let Some(host) = self.host(&source.context) else {
                continue;
            };
            let Some(source_diagnostics) = host.diagnostics(source.file).await else {
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
        diagnostics.sort_by_key(|diagnostic| {
            (
                diagnostic.path.clone(),
                diagnostic.range.start,
                diagnostic.range.end,
                diagnostic
                    .context
                    .as_ref()
                    .map(|context| context.id.clone()),
            )
        });
        diagnostics
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
    use std::time::{SystemTime, UNIX_EPOCH};

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
            .host_sources_for_document(&shared_id)
            .into_iter()
            .map(|source| source.context.0)
            .collect::<Vec<_>>();
        let api = analysis
            .host(&AnalysisContextId("api".to_string()))
            .unwrap();

        assert_eq!(analysis.host_count(), 2);
        assert!(
            analysis
                .host(&AnalysisContextId("shared".to_string()))
                .is_none()
        );
        assert_eq!(shared_contexts, vec!["api", "frontend"]);
        assert_eq!(api.catalog().default_schema, "app");
        assert_eq!(api.lint_options().unindexed_scan_severity, None);

        fs::remove_dir_all(root).unwrap();
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
}
