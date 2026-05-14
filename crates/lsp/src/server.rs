use crate::convert::{semantic_tokens_legend, to_lsp_diagnostic};
use crate::position::{byte_to_position, encode_semantic_tokens};
use dsql_frontend::{
    AnalysisHost, CompletionKind, DocumentDiagnostics, TextEdit as FrontendTextEdit, TextEditRange,
    TextPosition,
};
use std::{
    error::Error,
    fs::OpenOptions,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams, CompletionResponse,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentFormattingParams, Hover, HoverContents, HoverParams, InitializeParams,
    InitializeResult, InitializedParams, MarkupContent, MarkupKind, MessageType, OneOf, Position,
    Range, SemanticTokens, SemanticTokensFullOptions, SemanticTokensOptions, SemanticTokensParams,
    SemanticTokensResult, SemanticTokensServerCapabilities, ServerCapabilities,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit,
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
    analysis: AnalysisHost,
    project_catalog_loaded: AtomicBool,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            analysis: AnalysisHost::new(),
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
        match project.load_catalog() {
            Ok(catalog) => {
                info!(schema_dir = %project.schema.display(), "loaded catalog");
                self.analysis.set_catalog(catalog);
                self.project_catalog_loaded.store(true, Ordering::Release);
                self.client
                    .log_message(
                        MessageType::INFO,
                        format!("dsql loaded catalog from {}", project.schema.display()),
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

    async fn publish_diagnostics(&self, result: DocumentDiagnostics) {
        let uri = result.snapshot.uri.parse().ok();
        let lsp_diagnostics = result
            .diagnostics
            .iter()
            .map(|diagnostic| to_lsp_diagnostic(diagnostic, &result.snapshot.rope))
            .collect();
        if let Some(uri) = uri {
            self.client
                .publish_diagnostics(uri, lsp_diagnostics, Some(result.snapshot.version))
                .await;
        }
    }

    async fn publish_document_diagnostics(&self, results: Vec<DocumentDiagnostics>) {
        for result in results {
            self.publish_diagnostics(result).await;
        }
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        self.load_project_catalog().await;
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                document_formatting_provider: Some(OneOf::Left(true)),
                completion_provider: Some(CompletionOptions::default()),
                hover_provider: Some(
                    tower_lsp_server::lsp_types::HoverProviderCapability::Simple(true),
                ),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: semantic_tokens_legend(),
                            range: None,
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            ..SemanticTokensOptions::default()
                        },
                    ),
                ),
                ..ServerCapabilities::default()
            },
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
        self.analysis
            .open_document(uri.clone(), version, params.text_document.text)
            .await;
        self.publish_document_diagnostics(self.analysis.open_document_diagnostics().await)
            .await;
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
        let result = self
            .analysis
            .change_document(uri.to_string(), version, edits)
            .await;
        if result.is_some() {
            self.publish_document_diagnostics(self.analysis.open_document_diagnostics().await)
                .await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.analysis.close_document(&uri.to_string());
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;
        let Some(format) = self.analysis.document_format(&uri.to_string()).await else {
            return Ok(None);
        };
        if !format.formatted.diagnostics.is_empty() {
            return Ok(None);
        }
        let end = byte_to_position(&format.snapshot.rope, format.snapshot.rope.len());
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
        let Some(items) = self
            .analysis
            .completions(
                &uri.to_string(),
                TextPosition {
                    line: position.line,
                    character: position.character,
                },
            )
            .await
        else {
            return Ok(None);
        };

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
                        CompletionKind::Keyword => CompletionItemKind::KEYWORD,
                        CompletionKind::Operator => CompletionItemKind::OPERATOR,
                    }),
                    detail: item.detail,
                    ..CompletionItem::default()
                })
                .collect(),
        )))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some(info) = self
            .analysis
            .hover(
                &uri.to_string(),
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

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri;
        let Some(tokens) = self.analysis.semantic_tokens(&uri.to_string()).await else {
            return Ok(None);
        };
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: Some(tokens.snapshot.revision.0.to_string()),
            data: encode_semantic_tokens(&tokens.snapshot.rope, &tokens.tokens),
        })))
    }
}

fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let path = uri.strip_prefix("file://")?;
    if path.is_empty() {
        return None;
    }
    Some(PathBuf::from(percent_decode(path)))
}

fn init_lsp_logging() {
    let Some(path) = lsp_log_path() else {
        return;
    };
    let Ok(file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || {
            file.try_clone()
                .expect("failed to clone dsql lsp log file handle")
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

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            output.push(high << 4 | low);
            index += 3;
            continue;
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
