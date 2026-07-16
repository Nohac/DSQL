//! The language server: one bowl per session, live buffers as rope edits,
//! diagnostics published after every settle that follows a change.

use std::path::PathBuf;

use bowl::{Bowl, Entity, Mut, Query};
use ropey::Rope;
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::{
    CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams, CompletionResponse,
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentFormattingParams, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, InitializedParams, Location, MarkupContent, MarkupKind,
    MessageType, OneOf, Position as LspPosition, Range, SemanticTokensFullOptions,
    SemanticTokensOptions, SemanticTokensParams, SemanticTokensResult,
    SemanticTokensServerCapabilities, ServerCapabilities, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextEdit, Uri,
};
use tower_lsp_server::{Client, LanguageServer, LspService, Server};

use dsql_core::catalog::{Catalog, CatalogSourceRoot, insert_catalog};
use dsql_core::facts::{
    BelongsToFile, Diagnostic as DiagnosticFact, Severity, Span, arm_editor_demands,
};
use dsql_core::format::{FormatConfidence, format_document};
use dsql_core::grammar::parse;
use dsql_core::service::{
    CatalogDefinition, CompletionKind, CompletionList, CompletionRequest, DefinitionRequest,
    DefinitionTarget, HoverInfo, HoverRequest, Position as DsqlPosition, semantic_tokens,
};
use dsql_core::source::{
    BelongsToHost, ContentSpan, DsqlDocument, FilePath, HostProjection, OpenBuffer,
    ResolutionScope, SourceKind, SourceOffset, SourceText, insert_source_scoped,
};
use dsql_project::{Project, ProjectError, populate_project_bowl};

use crate::position::{
    byte_to_position, encode_semantic_tokens, position_to_byte, semantic_tokens_legend,
};

/// Serves the language server over stdio until the client disconnects.
pub async fn run_stdio() {
    serve(tokio::io::stdin(), tokio::io::stdout()).await;
}

pub(crate) async fn serve<I, O>(input: I, output: O)
where
    I: tokio::io::AsyncRead + Unpin,
    O: tokio::io::AsyncWrite,
{
    let (service, socket) = LspService::new(Backend::new);
    Server::new(input, output, socket).serve(service).await;
}

struct Backend {
    client: Client,
    /// The session bowl. Internally shared and locked by porridge; created
    /// empty here and populated (language, catalog, project documents) in
    /// `initialize`.
    bowl: Bowl,
    /// Serializes buffer *mutations*: tower-lsp runs handlers
    /// concurrently, and a FIFO-fair tokio mutex preserves the client's
    /// notification order — the property incremental `didChange` ranges
    /// depend on. Publishing happens after this lock drops, so a slow
    /// client never blocks edits. All session *state* lives in the bowl.
    session: tokio::sync::Mutex<()>,
    /// Orders diagnostic publication: scoop-and-send holds this FIFO
    /// lock, so an older handler's publication can never overtake a newer
    /// one — while edits (the `session` lock) never wait on client I/O.
    publisher: tokio::sync::Mutex<()>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            bowl: Bowl::builder().plugin(dsql_core::DsqlPlugin).build(),
            session: tokio::sync::Mutex::new(()),
            publisher: tokio::sync::Mutex::new(()),
        }
    }

    /// Publishes diagnostics for every open document. Open residency is
    /// bowl data — the `OpenBuffer` rows the editor stamps — so no
    /// adapter-side mirror of open files exists. Cheap when nothing
    /// changed (the bowl is already settled); needed because an edit in
    /// one file can move diagnostics in another (fragments resolve across
    /// files).
    async fn publish_open_documents(&self) {
        let _publisher = self.publisher.lock().await;
        self.publish_open_documents_locked().await;
    }

    /// [`Self::publish_open_documents`] body, for callers already holding
    /// the publisher lock.
    async fn publish_open_documents_locked(&self) {
        let open = self
            .bowl()
            .scoop::<Query<(Entity, &FilePath), bowl::With<OpenBuffer>>>()
            .await;
        let paths: Vec<String> = open
            .collect()
            .into_iter()
            .map(|(_, path)| path.0.clone())
            .collect();
        for path in paths {
            let Some(uri) = Uri::from_file_path(&path) else {
                continue;
            };
            self.publish_diagnostics(uri, &path).await;
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
            // LSP-owned buffers are never evicted; a non-resident rope
            // here would be a residency-policy bug upstream.
            .and_then(|(entity, source)| Some((entity, source.rope()?.clone())))
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
        let projection = HostProjection::new(
            regions
                .collect()
                .into_iter()
                .map(|(region, host, offset)| (region, host.0, offset.0)),
        );
        let offset_of = |file: Entity| -> Option<usize> {
            let (target, offset) = projection.target_of(file);
            (target == file_entity).then_some(offset)
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

/// Formats one standalone dsql document when parsing is clean and the
/// canonical text differs from the source.
fn format_changed_document(source: &str) -> Option<String> {
    let (cst, diagnostics) = parse(source);
    let formatted = format_document(&cst.into_data(), source, !diagnostics.is_empty());
    (formatted.confidence == FormatConfidence::Full && formatted.text != source)
        .then_some(formatted.text)
}

/// Formats the dsql body of one embedded region without disturbing the
/// whitespace that places it inside its host template.
fn format_changed_region(source: &str) -> Option<String> {
    let body_start = source.len() - source.trim_start().len();
    let body_end = source.trim_end().len();
    if body_start >= body_end {
        return None;
    }

    let leading = &source[..body_start];
    let trailing = &source[body_end..];
    let body = &source[body_start..body_end];
    let formatted = format_changed_document(body)?;
    let formatted = formatted.strip_suffix('\n').unwrap_or(&formatted);

    // A multiline template may indent its first dsql token relative to
    // the host. The standalone formatter owns dsql indentation; add only
    // that ambient prefix to continuation lines. Inline leading spaces do
    // not establish a multiline prefix.
    let ambient = leading
        .rsplit_once('\n')
        .map_or("", |(_, indentation)| indentation);
    let mut indented = String::with_capacity(formatted.len());
    for (index, line) in formatted.split('\n').enumerate() {
        if index > 0 {
            indented.push('\n');
            if !line.is_empty() {
                indented.push_str(ambient);
            }
        }
        indented.push_str(line);
    }

    let output = format!("{leading}{indented}{trailing}");
    (output != source).then_some(output)
}

fn text_edit(rope: &Rope, span: Span, new_text: String) -> TextEdit {
    TextEdit {
        range: Range {
            start: byte_to_position(rope, span.start),
            end: byte_to_position(rope, span.end),
        },
        new_text,
    }
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
        dsql_core::install_default_singletons(&self.bowl).await;
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
        arm_editor_demands(&self.bowl).await;

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
        let standalone_dsql = params.text_document.language_id == "dsql";
        let mut ambiguity_warning: Option<String> = None;
        let text = params.text_document.text;
        let session = self.session.lock().await;

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
            // A file the project loader has not seen: the bowl's scope
            // configuration says which scope owns its path, so a freshly
            // created document resolves its scope's imports — no
            // adapter-side project state.
            let scopes = self
                .bowl()
                .scoop::<Query<(Entity, &dsql_core::source::ScopeDocuments)>>()
                .await;
            let ownership = scopes
                .collect()
                .into_iter()
                .next()
                .map(|(_, documents)| documents.ownership_of(&path))
                .unwrap_or(dsql_core::source::ScopeOwnership::ImplicitDefault);
            use dsql_core::source::ScopeOwnership;
            let (scope, kind) = match ownership {
                ScopeOwnership::Unique(assignment) => {
                    (ResolutionScope(assignment.scope), assignment.kind)
                }
                ScopeOwnership::ImplicitDefault | ScopeOwnership::Unmatched if standalone_dsql => {
                    (ResolutionScope::default_scope(), SourceKind::Dsql)
                }
                ScopeOwnership::ImplicitDefault | ScopeOwnership::Unmatched => return,
                ScopeOwnership::Ambiguous(assignments) => {
                    // The warning sends after the mutation lock drops; a
                    // slow client must not block subsequent edits.
                    let choices: Vec<String> = assignments
                        .iter()
                        .map(|assignment| {
                            format!("{} via {}", assignment.scope, assignment.kind.resolver())
                        })
                        .collect();
                    ambiguity_warning = Some(format!(
                        "{path} has several document assignments ({}); using `{}` via `{}`",
                        choices.join(", "),
                        assignments[0].scope,
                        assignments[0].kind.resolver()
                    ));
                    (
                        ResolutionScope(assignments[0].scope.clone()),
                        assignments[0].kind.clone(),
                    )
                }
            };
            let entity = insert_source_scoped(self.bowl(), path.clone(), &text, scope, kind).await;
            self.bowl().entity(entity).insert((OpenBuffer,)).await;
        }

        drop(session);
        self.publish_open_documents().await;
        if let Some(warning) = ambiguity_warning {
            self.client
                .show_message(MessageType::WARNING, warning)
                .await;
        }
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
                        match (change.range, source.rope()) {
                            (Some(range), Some(rope)) => {
                                let start = position_to_byte(rope, range.start);
                                let end = position_to_byte(rope, range.end);
                                // Just observed resident; cannot fail.
                                source.apply_edit(start..end, &change.text).ok();
                            }
                            (Some(_), None) => {
                                // A ranged edit against an evicted rope
                                // has nothing to apply to, and its text is
                                // only a fragment — replacing the document
                                // with it would corrupt the buffer. Drop
                                // the edit; unreachable by policy (open
                                // buffers stay resident), and the next
                                // full sync self-heals.
                            }
                            (None, _) => source.set_text(&change.text),
                        }
                    }
                })
                .await;
        }

        drop(session);
        self.publish_open_documents().await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        // The editor no longer owns the buffer: the durable disk revision
        // becomes authoritative again, so reload it (an unsaved buffer's
        // text must not survive the editor discarding it). Unreadable
        // files keep the last text until disk watching lands.
        let Some(path) = uri_path(&params.text_document.uri) else {
            return;
        };
        let session = self.session.lock().await;
        if let Some((entity, _)) = self.rope_of(&path).await {
            if let Ok(disk) = tokio::fs::read_to_string(&path).await {
                let sources = self
                    .bowl()
                    .scoop::<Query<(Entity, Mut<SourceText>)>>()
                    .await;
                for (source_entity, source) in sources.collect() {
                    if source_entity == entity {
                        source
                            .with_latest(move |source| source.set_text(&disk))
                            .await;
                        break;
                    }
                }
            }
            self.bowl().entity(entity).remove::<OpenBuffer>().await;
        }
        drop(session);
        // The closed document leaves the open set, so the republish loop
        // no longer covers it: publish its disk-revision state once so
        // the client doesn't keep stale editor-buffer diagnostics.
        self.publish_diagnostics(params.text_document.uri, &path)
            .await;
        self.publish_open_documents().await;
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
            .insert((HoverRequest, FilePath(path), DsqlPosition { offset }))
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
            .insert((CompletionRequest, FilePath(path), DsqlPosition { offset }))
            .await
            .bind()
            .take::<CompletionList>()
            .await
        else {
            return Ok(None);
        };

        let replace_range = list.replace.map(|span| Range {
            start: byte_to_position(&rope, span.start),
            end: byte_to_position(&rope, span.end),
        });
        let items: Vec<CompletionItem> = list
            .items
            .iter()
            .map(|item| CompletionItem {
                label: item.label.clone(),
                kind: Some(match item.kind {
                    CompletionKind::Column => CompletionItemKind::FIELD,
                    CompletionKind::Relation => CompletionItemKind::REFERENCE,
                    CompletionKind::Table => CompletionItemKind::CLASS,
                    CompletionKind::Fragment => CompletionItemKind::SNIPPET,
                    CompletionKind::Directive => CompletionItemKind::FUNCTION,
                    CompletionKind::Scope => CompletionItemKind::OPERATOR,
                    CompletionKind::Operator => CompletionItemKind::OPERATOR,
                    CompletionKind::Keyword => CompletionItemKind::KEYWORD,
                }),
                detail: item.detail.clone(),
                text_edit: replace_range.map(|range| {
                    tower_lsp_server::ls_types::CompletionTextEdit::Edit(TextEdit {
                        range,
                        new_text: item
                            .insert_text
                            .clone()
                            .unwrap_or_else(|| item.label.clone()),
                    })
                }),
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
            .insert((DefinitionRequest, FilePath(path), DsqlPosition { offset }))
            .await
            .bind()
            .take::<DefinitionTarget>()
            .await
        else {
            return Ok(None);
        };

        let (file, span) = match target.as_ref() {
            DefinitionTarget::Source { file, span } => (*file, *span),
            DefinitionTarget::Catalog(target) => {
                return Ok(catalog_location(self.bowl(), target)
                    .await
                    .map(GotoDefinitionResponse::Scalar));
            }
        };

        // Resolve the target file entity back to its path and rope. A
        // target inside an extracted region resolves to its host file,
        // with the span shifted into host coordinates.
        let regions = self
            .bowl()
            .scoop::<Query<(Entity, &BelongsToHost, &SourceOffset)>>()
            .await;
        let projection = HostProjection::new(
            regions
                .collect()
                .into_iter()
                .map(|(region, host, offset)| (region, host.0, offset.0)),
        );
        let (target_file, offset) = projection.target_of(file);
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
                start: byte_to_position(&target_rope, offset + span.start),
                end: byte_to_position(&target_rope, offset + span.end),
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

        let tokens = semantic_tokens(self.bowl(), path).await;

        Ok(Some(SemanticTokensResult::Tokens(
            tower_lsp_server::ls_types::SemanticTokens {
                result_id: None,
                data: encode_semantic_tokens(&rope, &tokens),
            },
        )))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let Some(path) = uri_path(&params.text_document.uri) else {
            return Ok(None);
        };
        // Formatting edits are tied to one exact host revision. Serialize
        // the rope and derived-region reads against buffer mutations.
        let _session = self.session.lock().await;
        let Some((file_entity, rope)) = self.rope_of(&path).await else {
            return Ok(None);
        };

        let documents = self.bowl().scoop::<Query<(Entity, &DsqlDocument)>>().await;
        if documents
            .collect()
            .into_iter()
            .any(|(document, _)| document == file_entity)
        {
            let text = rope.to_string();
            return Ok(format_changed_document(&text).map(|formatted| {
                vec![text_edit(
                    &rope,
                    Span {
                        start: 0,
                        end: rope.len(),
                    },
                    formatted,
                )]
            }));
        }

        let regions = self
            .bowl()
            .scoop::<Query<(Entity, &BelongsToHost, &ContentSpan, &SourceText)>>()
            .await;
        let mut edits: Vec<(usize, TextEdit)> = regions
            .collect()
            .into_iter()
            .filter(|(_, host, _, _)| host.0 == file_entity)
            .filter_map(|(_, _, content, source)| {
                let text = source.to_text()?;
                let formatted = format_changed_region(&text)?;
                Some((content.0.start, text_edit(&rope, content.0, formatted)))
            })
            .collect();
        edits.sort_by_key(|(start, _)| *start);
        let edits: Vec<TextEdit> = edits.into_iter().map(|(_, edit)| edit).collect();
        Ok((!edits.is_empty()).then_some(edits))
    }
}

/// Presents a semantic catalog target as its YAML source location. The
/// schema root is session state owned by the bowl; this adapter only maps
/// it into LSP URI/range values.
async fn catalog_location(bowl: &Bowl, target: &CatalogDefinition) -> Option<Location> {
    let roots = bowl.scoop::<Query<(Entity, &CatalogSourceRoot)>>().await;
    let root = roots
        .collect()
        .into_iter()
        .next()
        .map(|(_, root)| root.0.clone())?;
    let (schema, table, column) = match target {
        CatalogDefinition::Table { schema, table } => (schema, table, None),
        CatalogDefinition::Column {
            schema,
            table,
            column,
        } => (schema, table, Some(column.as_str())),
    };
    let path = root.join(schema).join(format!("{table}.yaml"));
    let contents = tokio::fs::read_to_string(&path).await.ok()?;
    let line = column
        .and_then(|column| yaml_column_line(&contents, column))
        .or_else(|| yaml_table_line(&contents, table))
        .unwrap_or_default() as u32;
    let uri = Uri::from_file_path(&path)?;
    let position = LspPosition::new(line, 0);
    Some(Location {
        uri,
        range: Range {
            start: position,
            end: position,
        },
    })
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
