use crate::convert::semantic_tokens_legend;
use crate::position::{byte_to_position, encode_semantic_tokens};
use dsql_frontend::{
    CatalogDefinition, CompletionKind, DefinitionResult, PhysicalDocumentId, PresentedDiagnostic,
    ProjectHost, TextEdit as FrontendTextEdit, TextEditRange, TextPosition,
};
use std::str::FromStr;
use std::{
    error::Error,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::{
    CodeActionParams, CodeActionResponse, CompletionItem, CompletionItemKind, CompletionOptions,
    CompletionParams, CompletionResponse, Diagnostic, DiagnosticSeverity,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentFormattingParams, ExecuteCommandParams, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverContents, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
    InitializedParams, LSPAny, Location, MarkupContent, MarkupKind, MessageType, NumberOrString,
    OneOf, Position, Range, SemanticTokens, SemanticTokensFullOptions, SemanticTokensOptions,
    SemanticTokensParams, SemanticTokensResult, SemanticTokensServerCapabilities,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, Uri,
};
use tower_lsp_server::{Client, LanguageServer, LspService, Server};
use tracing::{info, warn};

pub async fn run_stdio() -> std::result::Result<(), Box<dyn Error + Send + Sync>> {
    init_lsp_logging();
    info!("starting dsql lsp");
    info!(
        cwd = %std::env::current_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|error| format!("<unavailable: {error}>")),
        log_path = %lsp_log_path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<unavailable>".to_string()),
        "lsp process context"
    );
    info!("project lookup starts from cwd and walks parents looking for dsql/dsql.toml");
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
    Ok(())
}

struct Backend {
    client: Client,
    project: ProjectHost,
    project_catalog_loaded: AtomicBool,
}

impl Backend {
    fn new(client: Client) -> Self {
        let project = ProjectHost::new();
        project.set_standalone_context("editor");
        Self {
            client,
            project,
            project_catalog_loaded: AtomicBool::new(false),
        }
    }

    async fn load_project_catalog(&self) {
        match dsql_project::Project::load() {
            Ok(project) => {
                info!(schema_dir = %project.schema.display(), "found dsql project");
                self.apply_project_catalog(project).await;
            }
            Err(error) => {
                warn!(error = ?error, "failed to find dsql project from cwd");
                let current_dir = std::env::current_dir().ok();
                self.client
                    .log_message(
                        MessageType::INFO,
                        format!(
                            "dsql using hardcoded catalog; no dsql/dsql.toml found from {}",
                            current_dir
                                .as_deref()
                                .map(Path::display)
                                .map(|path| path.to_string())
                                .unwrap_or_else(|| "<unknown cwd>".to_string())
                        ),
                    )
                    .await;
            }
        }
    }

    async fn apply_project_catalog(&self, project: dsql_project::Project) -> bool {
        match self.project.reload_from_project(&project) {
            Ok(()) => {
                let indexed_count = self.project.source_scopes().len();
                info!(schema_dir = %project.schema.display(), "loaded project analysis");
                self.project_catalog_loaded.store(true, Ordering::Release);
                self.client
                    .log_message(
                        MessageType::INFO,
                        format!(
                            "dsql loaded catalog from {} and indexed {indexed_count} documents",
                            project.schema.display()
                        ),
                    )
                    .await;
                true
            }
            Err(error) => {
                warn!(
                    schema_dir = %project.schema.display(),
                    error = ?error,
                    "failed to load catalog"
                );
                self.client
                    .log_message(
                        MessageType::WARNING,
                        format!(
                            "dsql failed to load catalog from {}: {error}",
                            project.schema.display()
                        ),
                    )
                    .await;
                false
            }
        }
    }

    async fn load_project_catalog_for_document(&self, uri: &str) {
        if self.project_catalog_loaded.load(Ordering::Acquire) {
            return;
        }
        let Some(path) = file_uri_to_path(uri).and_then(|path| parent_or_self(&path)) else {
            warn!(uri, "document fallback skipped; could not map uri to path");
            return;
        };
        let Some(project) = dsql_project::Project::try_load_from(&path) else {
            info!(path = %path.display(), "document fallback found no dsql project");
            return;
        };
        info!(
            path = %path.display(),
            schema_dir = %project.schema.display(),
            "document fallback found dsql project"
        );
        self.apply_project_catalog(project).await;
    }

    async fn publish_diagnostics_for_document(&self, document_id: &PhysicalDocumentId) {
        let Some(uri) = path_to_uri(&document_id.0) else {
            return;
        };
        let version = self
            .project
            .document(document_id)
            .map(|document| document.revision.0.min(i32::MAX as u64) as i32);
        let diagnostics = self
            .project
            .diagnostics_for_document(document_id)
            .await
            .iter()
            .map(to_lsp_presented_diagnostic)
            .collect();
        self.client
            .publish_diagnostics(uri, diagnostics, version)
            .await;
    }

    async fn publish_open_document_diagnostics(&self) {
        for document_id in self.project.open_documents() {
            self.publish_diagnostics_for_document(&document_id).await;
        }
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        self.load_project_catalog().await;
        let mut capabilities = ServerCapabilities {
            text_document_sync: Some(TextDocumentSyncCapability::Kind(
                TextDocumentSyncKind::INCREMENTAL,
            )),
            document_formatting_provider: Some(OneOf::Left(true)),
            completion_provider: Some(CompletionOptions {
                trigger_characters: Some(vec![
                    ".".to_string(),
                    "~".to_string(),
                    ":".to_string(),
                    "@".to_string(),
                ]),
                ..CompletionOptions::default()
            }),
            definition_provider: Some(OneOf::Left(true)),
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            semantic_tokens_provider: Some(
                SemanticTokensServerCapabilities::SemanticTokensOptions(SemanticTokensOptions {
                    legend: semantic_tokens_legend(),
                    range: None,
                    full: Some(SemanticTokensFullOptions::Bool(true)),
                    ..SemanticTokensOptions::default()
                }),
            ),
            ..ServerCapabilities::default()
        };
        crate::debug::configure_capabilities(&mut capabilities);
        Ok(InitializeResult {
            capabilities,
            ..InitializeResult::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "dsql language server initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        let uri = uri.to_string();
        self.load_project_catalog_for_document(&uri).await;
        if let Some(path) = file_uri_to_path(&uri) {
            let document_id = PhysicalDocumentId(path.clone());
            self.project
                .open_document(document_id, Some(path), version, params.text_document.text);
            self.publish_open_document_diagnostics().await;
        }
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        let edits = params
            .content_changes
            .into_iter()
            .map(|change| FrontendTextEdit {
                range: change.range.map(|range| TextEditRange {
                    start: TextPosition {
                        line: range.start.line,
                        character: range.start.character,
                    },
                    end: TextPosition {
                        line: range.end.line,
                        character: range.end.character,
                    },
                }),
                text: change.text,
            })
            .collect();
        if let Some(path) = file_uri_to_path(uri.as_str()) {
            let document_id = PhysicalDocumentId(path);
            if self
                .project
                .change_document(&document_id, version, edits)
                .is_some()
            {
                self.publish_open_document_diagnostics().await;
            }
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        let uri_string = uri.to_string();
        if let Some(path) = file_uri_to_path(&uri_string) {
            let document_id = PhysicalDocumentId(path.clone());
            self.project.close_document(&document_id);
            if let Some(start) = parent_or_self(&path)
                && let Some(project) = dsql_project::Project::try_load_from(&start)
            {
                self.apply_project_catalog(project).await;
            }
        }
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;
        let uri_string = uri.to_string();
        info!(uri = %uri_string, "formatting request");
        let Some(document_id) = file_uri_to_path(&uri_string).map(PhysicalDocumentId) else {
            return Ok(None);
        };
        let Some(format) = self.project.document_format(&document_id).await else {
            info!(uri = %uri_string, "formatting skipped; no format result");
            return Ok(None);
        };
        if !format.formatted.diagnostics.is_empty() {
            info!(
                uri = %uri_string,
                diagnostic_count = format.formatted.diagnostics.len(),
                "formatting skipped; formatter returned diagnostics"
            );
            return Ok(None);
        }
        let end = byte_to_position(&format.snapshot.rope, format.snapshot.rope.len());
        info!(
            uri = %uri_string,
            original_bytes = format.snapshot.rope.len(),
            formatted_bytes = format.formatted.text.len(),
            "formatting returning full document edit"
        );
        Ok(Some(vec![TextEdit {
            range: Range {
                start: Position::new(0, 0),
                end,
            },
            new_text: format.formatted.text,
        }]))
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let uri_string = uri.to_string();
        let text_position = TextPosition {
            line: position.line,
            character: position.character,
        };
        let Some(document_id) = file_uri_to_path(&uri_string).map(PhysicalDocumentId) else {
            return Ok(None);
        };
        let request_byte = self
            .project
            .document_byte_offset(&document_id, text_position);
        let request_context = request_byte
            .and_then(|byte| completion_source_context(&self.project, &document_id, byte));
        let Some(items) = self.project.completions(&document_id, text_position).await else {
            info!(
                uri = uri.as_str(),
                line = position.line,
                character = position.character,
                byte = request_byte,
                context = request_context.as_deref().unwrap_or("<unavailable>"),
                "completion request returned no document"
            );
            return Ok(None);
        };

        info!(
            uri = uri.as_str(),
            line = position.line,
            character = position.character,
            byte = request_byte,
            context = request_context.as_deref().unwrap_or("<unavailable>"),
            count = items.len(),
            labels = %items
                .iter()
                .take(30)
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            "completion request"
        );

        Ok(Some(CompletionResponse::Array(
            items
                .into_iter()
                .map(|item| CompletionItem {
                    label: item.label,
                    kind: Some(match item.kind {
                        CompletionKind::Table => CompletionItemKind::CLASS,
                        CompletionKind::Column => CompletionItemKind::FIELD,
                        CompletionKind::Relation => CompletionItemKind::REFERENCE,
                        CompletionKind::Fragment => CompletionItemKind::MODULE,
                        CompletionKind::Directive => CompletionItemKind::FUNCTION,
                        CompletionKind::Keyword => CompletionItemKind::KEYWORD,
                        CompletionKind::Operator => CompletionItemKind::OPERATOR,
                    }),
                    detail: item.detail,
                    insert_text: item.insert_text,
                    ..CompletionItem::default()
                })
                .collect(),
        )))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some(document_id) = file_uri_to_path(uri.as_str()).map(PhysicalDocumentId) else {
            return Ok(None);
        };
        let Some(info) = self
            .project
            .hover(
                &document_id,
                TextPosition {
                    line: position.line,
                    character: position.character,
                },
            )
            .await
        else {
            return Ok(None);
        };

        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: info.markdown,
            }),
            range: None,
        }))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some(document_id) = file_uri_to_path(uri.as_str()).map(PhysicalDocumentId) else {
            return Ok(None);
        };
        let Some(definition) = self
            .project
            .definition(
                &document_id,
                TextPosition {
                    line: position.line,
                    character: position.character,
                },
            )
            .await
        else {
            return Ok(None);
        };

        let location = match definition {
            DefinitionResult::Source(source) => {
                let Some((uri, rope)) = source_definition_uri_and_rope(&self.project, &source)
                else {
                    return Ok(None);
                };
                Location::new(
                    uri,
                    Range {
                        start: byte_to_position(&rope, source.range.start as usize),
                        end: byte_to_position(&rope, source.range.end as usize),
                    },
                )
            }
            DefinitionResult::Catalog(target) => {
                let Some(path) =
                    file_uri_to_path(uri.as_str()).and_then(|path| parent_or_self(&path))
                else {
                    return Ok(None);
                };
                let Some(project) = dsql_project::Project::try_load_from(&path) else {
                    return Ok(None);
                };
                let Some(location) = catalog_location(&project.schema, &target) else {
                    return Ok(None);
                };
                location
            }
        };

        Ok(Some(GotoDefinitionResponse::Scalar(location)))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri;
        let Some(document_id) = file_uri_to_path(uri.as_str()).map(PhysicalDocumentId) else {
            return Ok(None);
        };
        let Some(tokens) = self.project.semantic_tokens(&document_id).await else {
            return Ok(None);
        };
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: Some(tokens.snapshot.revision.0.to_string()),
            data: encode_semantic_tokens(&tokens.snapshot.rope, &tokens.tokens),
        })))
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        crate::debug::code_action(params).await
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<LSPAny>> {
        crate::debug::execute_command(&self.client, params).await
    }
}

fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    Uri::from_str(uri)
        .ok()?
        .to_file_path()
        .map(|path| path.into_owned())
}

fn to_lsp_presented_diagnostic(diagnostic: &PresentedDiagnostic) -> Diagnostic {
    let message = if let Some(context) = &diagnostic.context {
        format!("[{}] {}", context.label, diagnostic.diagnostic.message)
    } else {
        diagnostic.diagnostic.message.clone()
    };
    Diagnostic {
        range: Range {
            start: Position::new(
                diagnostic.start_position.line,
                diagnostic.start_position.character,
            ),
            end: Position::new(
                diagnostic.end_position.line,
                diagnostic.end_position.character,
            ),
        },
        severity: Some(match diagnostic.diagnostic.severity {
            dsql_core::Severity::Error => DiagnosticSeverity::ERROR,
            dsql_core::Severity::Warning => DiagnosticSeverity::WARNING,
            dsql_core::Severity::Info => DiagnosticSeverity::INFORMATION,
        }),
        code: Some(NumberOrString::String(format!(
            "{:?}",
            diagnostic.diagnostic.code
        ))),
        source: Some("dsql".to_string()),
        message,
        ..Diagnostic::default()
    }
}

fn source_definition_uri_and_rope(
    project: &ProjectHost,
    source: &dsql_frontend::SourceDefinition,
) -> Option<(Uri, ropey::Rope)> {
    let path = file_uri_to_path(&source.uri).unwrap_or_else(|| PathBuf::from(&source.uri));
    let uri = path_to_uri(&path)?;
    let rope = project
        .document_snapshot(&PhysicalDocumentId(path.clone()))
        .map(|snapshot| snapshot.rope)
        .or_else(|| {
            File::open(&path)
                .ok()
                .and_then(|file| ropey::Rope::from_reader(file).ok())
        })?;
    Some((uri, rope))
}

fn completion_source_context(
    project: &ProjectHost,
    document_id: &PhysicalDocumentId,
    byte: usize,
) -> Option<String> {
    let snapshot = project.document_snapshot(document_id)?;
    let start = byte.saturating_sub(80);
    let end = (byte + 80).min(snapshot.rope.len());
    let before = snapshot.rope.slice(start..byte).to_string();
    let after = snapshot.rope.slice(byte..end).to_string();
    Some(format!(
        "{}<cursor>{}",
        normalize_log_context(&before),
        normalize_log_context(&after)
    ))
}

fn normalize_log_context(value: &str) -> String {
    value.replace('\n', "\\n").replace('\r', "\\r")
}

fn catalog_location(schema_dir: &Path, target: &CatalogDefinition) -> Option<Location> {
    let (schema, table, column) = match target {
        CatalogDefinition::Table { schema, table } => (schema.as_str(), table.as_str(), None),
        CatalogDefinition::Column {
            schema,
            table,
            column,
        } => (schema.as_str(), table.as_str(), Some(column.as_str())),
    };
    let path = schema_dir.join(schema).join(format!("{table}.yaml"));
    let contents = fs::read_to_string(&path).ok()?;
    let range = catalog_yaml_range(&contents, table, column);
    Some(Location::new(path_to_uri(&path)?, range))
}

fn catalog_yaml_range(contents: &str, table: &str, column: Option<&str>) -> Range {
    let line = column
        .and_then(|column| yaml_column_line(contents, column))
        .or_else(|| yaml_table_line(contents, table))
        .unwrap_or(0);
    Range::new(Position::new(line as u32, 0), Position::new(line as u32, 0))
}

fn yaml_table_line(contents: &str, table: &str) -> Option<usize> {
    let expected = format!("name: {table}");
    contents
        .lines()
        .position(|line| line.trim() == expected && !line.trim_start().starts_with("- "))
}

fn yaml_column_line(contents: &str, column: &str) -> Option<usize> {
    let expected = format!("- name: {column}");
    contents.lines().position(|line| line.trim() == expected)
}

fn path_to_uri(path: &Path) -> Option<Uri> {
    Uri::from_file_path(path)
}

fn init_lsp_logging() {
    let Some(path) = lsp_log_path() else {
        return;
    };
    let Ok(unit_id) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || {
            unit_id
                .try_clone()
                .expect("failed to clone dsql lsp log unit_id handle")
        })
        .try_init();
}

fn lsp_log_path() -> Option<PathBuf> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent()?.parent()?;
    Some(workspace_root.join("lsp.log"))
}

fn parent_or_self(path: &Path) -> Option<PathBuf> {
    if path.is_dir() {
        Some(path.to_path_buf())
    } else {
        path.parent().map(Path::to_path_buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_yaml_range_targets_column_lines() {
        let yaml = "\
---
schema: public
name: movie_info
object_type: table
columns:
  - name: id
    data_type: int
  - name: note
    data_type: text
";

        let table = catalog_yaml_range(yaml, "movie_info", None);
        let column = catalog_yaml_range(yaml, "movie_info", Some("note"));

        assert_eq!(table.start, Position::new(2, 0));
        assert_eq!(column.start, Position::new(7, 0));
    }

    #[test]
    fn source_definition_plain_path_returns_file_uri() {
        let root = std::env::temp_dir().join(format!(
            "dsql-lsp-uri-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("fragments.dsql");
        fs::write(&path, "fragment TitleFields on titles { id }").unwrap();

        let source = dsql_frontend::SourceDefinition {
            uri: path.display().to_string(),
            range: dsql_core::TextRange::new(0, 8),
            kind: dsql_frontend::SourceDefinitionKind::Fragment,
        };

        let (uri, rope) = source_definition_uri_and_rope(&ProjectHost::new(), &source).unwrap();

        assert_eq!(uri.scheme().as_str(), "file");
        assert_eq!(
            file_uri_to_path(uri.as_str()).as_deref(),
            Some(path.as_path())
        );
        assert_eq!(rope.to_string(), "fragment TitleFields on titles { id }");

        fs::remove_dir_all(root).unwrap();
    }
}
