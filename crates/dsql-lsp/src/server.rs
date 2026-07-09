//! The language server: one bowl per session, live buffers as rope edits,
//! diagnostics published after every settle that follows a change.

use std::path::PathBuf;

use bowl::{Bowl, Entity, Mut, Query, Singleton};
use ropey::Rope;
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::{
    CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams, CompletionResponse,
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentFormattingParams, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, InitializedParams, Location, MarkupContent, MarkupKind,
    MessageType, OneOf, Range, SemanticTokensFullOptions, SemanticTokensOptions,
    SemanticTokensParams, SemanticTokensResult, SemanticTokensServerCapabilities,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, Uri,
};
use tower_lsp_server::{Client, LanguageServer, LspService, Server};

use dsql_core::catalog::{Catalog, insert_catalog};
use dsql_core::facts::{
    BelongsToFile, Diagnostic as DiagnosticFact, DiagnosticsDemand, Severity, Span, VariablesDemand,
};
use dsql_core::format::{FormatConfidence, format_document};
use dsql_core::grammar::parse;
use dsql_core::register_language;
use dsql_core::service::{
    CompletionKind, CompletionList, CompletionRequest, DefinitionRequest, DefinitionTarget,
    HoverInfo, HoverRequest, Position, SemanticTokens, SemanticTokensRequest,
};
use dsql_core::source::{
    BelongsToHost, FilePath, OpenBuffer, ResolutionScope, SourceOffset, SourceText,
    insert_source_scoped,
};
use dsql_project::{Project, ProjectError, populate_project_bowl};

use crate::position::{
    byte_to_position, encode_semantic_tokens, position_to_byte, semantic_tokens_legend,
};

/// Serves the language server over stdio until the client disconnects.
pub async fn run_stdio() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}

struct Backend {
    client: Client,
    /// The session bowl. Internally shared and locked by porridge; created
    /// empty here and populated (language, catalog, project documents) in
    /// `initialize`.
    bowl: Bowl,
    /// Serializes buffer mutations and diagnostic publishing. tower-lsp
    /// runs handlers concurrently; a tokio mutex is FIFO-fair, so locking
    /// at handler entry preserves the client's notification order — the
    /// property incremental `didChange` ranges depend on.
    session: tokio::sync::Mutex<SessionState>,
}

/// Editor-session bookkeeping guarded by the session lock.
#[derive(Default)]
struct SessionState {
    /// URIs of currently open documents, for cross-file diagnostic
    /// republishing: an edit in one file can change diagnostics anchored
    /// in any other (fragments resolve across files).
    open: std::collections::HashMap<String, Uri>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            bowl: Bowl::new(),
            session: tokio::sync::Mutex::new(SessionState::default()),
        }
    }

    /// Publishes diagnostics for every open document. Cheap when nothing
    /// changed (the bowl is already settled); needed because an edit in
    /// one file can move diagnostics in another.
    async fn publish_open_documents(&self, state: &SessionState) {
        for (path, uri) in &state.open {
            self.publish_diagnostics(uri.clone(), path).await;
        }
    }

    fn bowl(&self) -> &Bowl {
        &self.bowl
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
        // A host file's diagnostics live on its extracted regions; they
        // publish under the host, shifted into host coordinates.
        let regions = self
            .bowl()
            .scoop::<Query<(Entity, &BelongsToHost, &SourceOffset)>>()
            .await;
        let region_offsets: Vec<(Entity, usize)> = regions
            .collect()
            .into_iter()
            .filter(|(_, host, _)| host.0 == file_entity)
            .map(|(region, _, offset)| (region, offset.0))
            .collect();
        let offset_of = |file: Entity| -> Option<usize> {
            if file == file_entity {
                return Some(0);
            }
            region_offsets
                .iter()
                .find(|(region, _)| *region == file)
                .map(|(_, offset)| *offset)
        };
        let mut diagnostics: Vec<Diagnostic> = rows
            .collect()
            .into_iter()
            .filter_map(|(entity, severity, span, diagnostic, file)| {
                offset_of(file.0).map(|offset| (entity, severity, span, diagnostic, offset))
            })
            .map(|(_, severity, span, diagnostic, offset)| Diagnostic {
                range: Range {
                    start: byte_to_position(&rope, offset + span.start),
                    end: byte_to_position(&rope, offset + span.end),
                },
                severity: Some(match severity {
                    Severity::Error => DiagnosticSeverity::ERROR,
                    Severity::Warning => DiagnosticSeverity::WARNING,
                    Severity::Info => DiagnosticSeverity::INFORMATION,
                }),
                message: diagnostic.0.clone(),
                source: Some("dsql".to_string()),
                ..Diagnostic::default()
            })
            .collect();
        diagnostics.sort_by_key(|diagnostic| {
            (
                diagnostic.range.start.line,
                diagnostic.range.start.character,
            )
        });

        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;
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

        // Populate the session bowl: project contents when a dsql.toml is
        // in reach, a bare language with an empty catalog otherwise, so
        // single files still parse and hover. A project that exists but
        // fails to load degrades the same way — but says so, loudly:
        // silently dropping the catalog and documents is indistinguishable
        // from "everything is broken" in the editor.
        register_language(&self.bowl).await;
        let populated = match Project::load_from(&start_dir).await {
            Ok(project) => match populate_project_bowl(&self.bowl, &project).await {
                Ok(()) => true,
                Err(error) => {
                    self.client
                        .show_message(
                            MessageType::ERROR,
                            format!("dsql project failed to load: {error}"),
                        )
                        .await;
                    false
                }
            },
            Err(error @ ProjectError::MissingRoot(_)) => {
                // Legitimately projectless: single-file mode.
                self.client
                    .log_message(MessageType::INFO, format!("dsql: {error}"))
                    .await;
                false
            }
            Err(error) => {
                self.client
                    .show_message(
                        MessageType::ERROR,
                        format!("dsql project failed to load: {error}"),
                    )
                    .await;
                false
            }
        };
        if !populated {
            insert_catalog(
                &self.bowl,
                Catalog {
                    default_schema: Catalog::DEFAULT_SCHEMA.to_string(),
                    schemas: Vec::new(),
                    tables: Vec::new(),
                    columns: Vec::new(),
                    foreign_keys: Vec::new(),
                },
            )
            .await;
        }
        self.bowl
            .insert((Singleton::<DiagnosticsDemand>::new(), DiagnosticsDemand))
            .await;
        self.bowl
            .insert((Singleton::<VariablesDemand>::new(), VariablesDemand))
            .await;

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![
                        ".".to_string(),
                        "$".to_string(),
                        "@".to_string(),
                    ]),
                    ..CompletionOptions::default()
                }),
                definition_provider: Some(OneOf::Left(true)),
                document_formatting_provider: Some(OneOf::Left(true)),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: semantic_tokens_legend(),
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

    async fn initialized(&self, _params: InitializedParams) {}

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let Some(path) = uri_path(&params.text_document.uri) else {
            return;
        };
        // Editors send didOpen for every restored tab; only dsql documents
        // and embedding hosts belong in the bowl. Anything else would be
        // parsed as dsql — error-recovering through a lockfile burns
        // seconds and floods the session with junk diagnostics.
        if !std::path::Path::new(&path)
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| matches!(ext, "dsql" | "ts" | "tsx"))
        {
            return;
        }
        let text = params.text_document.text;
        let mut session = self.session.lock().await;

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
            let entity = insert_source_scoped(
                self.bowl(),
                path.clone(),
                &text,
                ResolutionScope::default_scope(),
            )
            .await;
            self.bowl().entity(entity).insert((OpenBuffer,)).await;
        }

        session.open.insert(path, params.text_document.uri);
        self.publish_open_documents(&session).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let Some(path) = uri_path(&params.text_document.uri) else {
            return;
        };
        let session = self.session.lock().await;

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

        self.publish_open_documents(&session).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        // The buffer's last text stays authoritative until disk watching
        // lands; the editor no longer owns it, so retract the marker.
        let Some(path) = uri_path(&params.text_document.uri) else {
            return;
        };
        let mut session = self.session.lock().await;
        session.open.remove(&path);
        if let Some((entity, _)) = self.rope_of(&path).await {
            self.bowl().entity(entity).remove::<OpenBuffer>().await;
        }
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

        // Scaffold-priority answers mean nothing answered: no popup.
        Ok(info
            .ok()
            .filter(|info| info.priority > dsql_core::service::priority::RESOLVED)
            .map(|info| Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: info.text.clone(),
                }),
                range: None,
            }))
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let Some(path) = uri_path(&uri) else {
            return Ok(None);
        };
        let Some((_, rope)) = self.rope_of(&path).await else {
            return Ok(None);
        };
        let offset = position_to_byte(&rope, params.text_document_position.position);

        let Ok(list) = self
            .bowl()
            .insert((CompletionRequest, FilePath(path), Position { offset }))
            .await
            .bind()
            .take::<CompletionList>()
            .await
        else {
            return Ok(None);
        };

        let items: Vec<CompletionItem> = list
            .0
            .iter()
            .map(|item| CompletionItem {
                label: item.label.clone(),
                kind: Some(match item.kind {
                    CompletionKind::Column => CompletionItemKind::FIELD,
                    CompletionKind::Relation => CompletionItemKind::REFERENCE,
                    CompletionKind::Table => CompletionItemKind::CLASS,
                    CompletionKind::Fragment => CompletionItemKind::SNIPPET,
                    CompletionKind::Scope => CompletionItemKind::OPERATOR,
                    CompletionKind::Operator => CompletionItemKind::OPERATOR,
                    CompletionKind::Keyword => CompletionItemKind::KEYWORD,
                }),
                detail: item.detail.clone(),
                insert_text: item.insert_text.clone(),
                ..CompletionItem::default()
            })
            .collect();

        Ok(Some(CompletionResponse::Array(items)))
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

        // Resolve the target file entity back to its path and rope. A
        // target inside an extracted region resolves to its host file,
        // with the span shifted into host coordinates.
        let regions = self
            .bowl()
            .scoop::<Query<(Entity, &BelongsToHost, &SourceOffset)>>()
            .await;
        let (target_file, offset) = regions
            .collect()
            .into_iter()
            .find(|(entity, _, _)| *entity == target.file)
            .map_or((target.file, 0), |(_, host, offset)| (host.0, offset.0));
        let paths = self.bowl().scoop::<Query<(Entity, &FilePath)>>().await;
        let Some(target_path) = paths
            .collect()
            .into_iter()
            .find(|(entity, _)| *entity == target_file)
            .map(|(_, path)| path.0.clone())
        else {
            return Ok(None);
        };
        let Some((_, target_rope)) = self.rope_of(&target_path).await else {
            return Ok(None);
        };
        let Some(target_uri) = Uri::from_file_path(&target_path) else {
            return Ok(None);
        };

        Ok(Some(GotoDefinitionResponse::Scalar(Location {
            uri: target_uri,
            range: Range {
                start: byte_to_position(&target_rope, offset + target.span.start),
                end: byte_to_position(&target_rope, offset + target.span.end),
            },
        })))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let Some(path) = uri_path(&params.text_document.uri) else {
            return Ok(None);
        };
        let Some((_, rope)) = self.rope_of(&path).await else {
            return Ok(None);
        };

        let Ok(tokens) = self
            .bowl()
            .insert((SemanticTokensRequest, FilePath(path)))
            .await
            .bind()
            .take::<SemanticTokens>()
            .await
        else {
            return Ok(None);
        };

        Ok(Some(SemanticTokensResult::Tokens(
            tower_lsp_server::ls_types::SemanticTokens {
                result_id: None,
                data: encode_semantic_tokens(&rope, &tokens.0),
            },
        )))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let Some(path) = uri_path(&params.text_document.uri) else {
            return Ok(None);
        };
        // Only dsql documents format; host sources are another language's.
        if std::path::Path::new(&path)
            .extension()
            .and_then(|ext| ext.to_str())
            != Some("dsql")
        {
            return Ok(None);
        }
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
