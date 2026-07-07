//! The language server: one bowl per session, live buffers as rope edits,
//! diagnostics published after every settle that follows a change.

use std::path::PathBuf;
use std::sync::OnceLock;

use bowl::{Bowl, Entity, Mut, Query, Singleton};
use ropey::Rope;
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::{
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentFormattingParams, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, InitializedParams, Location, MarkupContent, MarkupKind,
    OneOf, Range, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit,
    Uri,
};
use tower_lsp_server::{Client, LanguageServer, LspService, Server};

use dsql_core::catalog::{Catalog, insert_catalog};
use dsql_core::facts::{
    BelongsToFile, Diagnostic as DiagnosticFact, DiagnosticsDemand, Severity, Span, VariablesDemand,
};
use dsql_core::format::{FormatConfidence, format_document};
use dsql_core::grammar::parse;
use dsql_core::register_language;
use dsql_core::service::{DefinitionRequest, DefinitionTarget, HoverInfo, HoverRequest, Position};
use dsql_core::source::{FilePath, OpenBuffer, SourceText};
use dsql_project::{Project, open_project_bowl};

use crate::position::{byte_to_position, position_to_byte};

/// Serves the language server over stdio until the client disconnects.
pub async fn run_stdio() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}

struct Backend {
    client: Client,
    bowl: OnceLock<Bowl>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            bowl: OnceLock::new(),
        }
    }

    fn bowl(&self) -> &Bowl {
        self.bowl
            .get()
            .expect("initialize runs before any other request")
    }

    /// Reads the current rope of `path`'s file entity, if it exists.
    async fn rope_of(&self, path: &str) -> Option<(Entity, Rope)> {
        let sources = self
            .bowl()
            .scoop::<Query<(Entity, &SourceText), bowl::Where<bowl::Eq<FilePath>>>>()
            .args(FilePath(path.to_string()))
            .await;
        sources
            .collect()
            .into_iter()
            .next()
            .map(|(entity, source)| (entity, source.rope().clone()))
    }

    /// Publishes the diagnostics currently derived for `path`.
    async fn publish_diagnostics(&self, uri: Uri, path: &str) {
        let Some((file_entity, rope)) = self.rope_of(path).await else {
            return;
        };
        let rows = self
            .bowl()
            .scoop::<Query<(Entity, &Severity, &Span, &DiagnosticFact, &BelongsToFile)>>()
            .await;
        let mut diagnostics: Vec<Diagnostic> = rows
            .collect()
            .into_iter()
            .filter(|(_, _, _, _, file)| file.0 == file_entity)
            .map(|(_, severity, span, diagnostic, _)| Diagnostic {
                range: Range {
                    start: byte_to_position(&rope, span.start),
                    end: byte_to_position(&rope, span.end),
                },
                severity: Some(match severity {
                    Severity::Error => DiagnosticSeverity::ERROR,
                    Severity::Warning => DiagnosticSeverity::WARNING,
                }),
                message: diagnostic.0.clone(),
                source: Some("dsql".to_string()),
                ..Diagnostic::default()
            })
            .collect();
        diagnostics.sort_by_key(|diagnostic| (diagnostic.range.start.line, diagnostic.range.start.character));

        self.client.publish_diagnostics(uri, diagnostics, None).await;
    }
}

fn uri_path(uri: &Uri) -> Option<String> {
    uri.to_file_path()
        .map(|path| path.into_owned().display().to_string())
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let start_dir = params
            .workspace_folders
            .as_ref()
            .and_then(|folders| folders.first())
            .and_then(|folder| uri_path(&folder.uri))
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));

        // A project bowl when a dsql.toml is in reach; a bare language bowl
        // with an empty catalog otherwise, so single files still parse and
        // hover.
        let bowl = match Project::load_from(&start_dir) {
            Ok(project) => open_project_bowl(&project)
                .await
                .unwrap_or_else(|_| Bowl::new()),
            Err(_) => {
                let bowl = Bowl::new();
                register_language(&bowl).await;
                insert_catalog(
                    &bowl,
                    Catalog {
                        default_schema: Catalog::DEFAULT_SCHEMA.to_string(),
                        schemas: Vec::new(),
                        tables: Vec::new(),
                        columns: Vec::new(),
                        foreign_keys: Vec::new(),
                    },
                )
                .await;
                bowl
            }
        };
        bowl.insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
            .await;
        bowl.insert((Singleton::<VariablesDemand>::new(), VariablesDemand))
            .await;
        let _ = self.bowl.set(bowl);

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                document_formatting_provider: Some(OneOf::Left(true)),
                ..ServerCapabilities::default()
            },
            ..InitializeResult::default()
        })
    }

    async fn initialized(&self, _params: InitializedParams) {}

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let Some(path) = uri_path(&params.text_document.uri) else {
            return;
        };
        let text = params.text_document.text;

        if let Some((entity, _)) = self.rope_of(&path).await {
            let sources = self
                .bowl()
                .scoop::<Query<(Entity, Mut<SourceText>)>>()
                .await;
            for (source_entity, source) in sources.collect() {
                if source_entity == entity {
                    source.with_latest(|source| source.set_text(&text)).await;
                }
            }
            self.bowl().entity(entity).insert((OpenBuffer,)).await;
        } else {
            self.bowl()
                .insert((FilePath(path.clone()), SourceText::from_text(&text), OpenBuffer))
                .await;
        }

        self.publish_diagnostics(params.text_document.uri, &path).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let Some(path) = uri_path(&params.text_document.uri) else {
            return;
        };

        let sources = self
            .bowl()
            .scoop::<Query<(Entity, Mut<SourceText>), bowl::Where<bowl::Eq<FilePath>>>>()
            .args(FilePath(path.clone()))
            .await;
        for (_, source) in sources.collect() {
            let changes = params.content_changes.clone();
            source
                .with_latest(move |source| {
                    for change in &changes {
                        match change.range {
                            Some(range) => {
                                let start = position_to_byte(source.rope(), range.start);
                                let end = position_to_byte(source.rope(), range.end);
                                source.apply_edit(start..end, &change.text);
                            }
                            None => source.set_text(&change.text),
                        }
                    }
                })
                .await;
        }

        self.publish_diagnostics(params.text_document.uri, &path).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        // The buffer's last text stays authoritative. Retracting the
        // `OpenBuffer` marker needs an external component-removal API that
        // porridge does not expose yet; nothing consumes the marker until
        // disk watching lands, so closing is otherwise a no-op.
        let _ = params;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let Some(path) = uri_path(&uri) else {
            return Ok(None);
        };
        let Some((_, rope)) = self.rope_of(&path).await else {
            return Ok(None);
        };
        let offset = position_to_byte(&rope, params.text_document_position_params.position);

        let info = self
            .bowl()
            .insert((HoverRequest, FilePath(path), Position { offset }))
            .await
            .bind()
            .take::<HoverInfo>()
            .await;

        Ok(info.ok().map(|info| Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: info.0.clone(),
            }),
            range: None,
        }))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let Some(path) = uri_path(&uri) else {
            return Ok(None);
        };
        let Some((_, rope)) = self.rope_of(&path).await else {
            return Ok(None);
        };
        let offset = position_to_byte(&rope, params.text_document_position_params.position);

        let Ok(target) = self
            .bowl()
            .insert((DefinitionRequest, FilePath(path), Position { offset }))
            .await
            .bind()
            .take::<DefinitionTarget>()
            .await
        else {
            return Ok(None);
        };

        // Resolve the target file entity back to its path and rope.
        let paths = self.bowl().scoop::<Query<(Entity, &FilePath)>>().await;
        let Some(target_path) = paths
            .collect()
            .into_iter()
            .find(|(entity, _)| *entity == target.file)
            .map(|(_, path)| path.0.clone())
        else {
            return Ok(None);
        };
        let Some((_, target_rope)) = self.rope_of(&target_path).await else {
            return Ok(None);
        };
        let target_uri = format!("file://{target_path}").parse::<Uri>().ok();
        let Some(target_uri) = target_uri else {
            return Ok(None);
        };

        Ok(Some(GotoDefinitionResponse::Scalar(Location {
            uri: target_uri,
            range: Range {
                start: byte_to_position(&target_rope, target.span.start),
                end: byte_to_position(&target_rope, target.span.end),
            },
        })))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let Some(path) = uri_path(&params.text_document.uri) else {
            return Ok(None);
        };
        let Some((_, rope)) = self.rope_of(&path).await else {
            return Ok(None);
        };
        let text = rope.to_string();
        let (cst, diagnostics) = parse(&text);
        let formatted = format_document(&cst.into_data(), &text, !diagnostics.is_empty());
        if formatted.confidence == FormatConfidence::PreserveOriginal || formatted.text == text {
            return Ok(None);
        }
        Ok(Some(vec![TextEdit {
            range: Range {
                start: byte_to_position(&rope, 0),
                end: byte_to_position(&rope, rope.len()),
            },
            new_text: formatted.text,
        }]))
    }
}
