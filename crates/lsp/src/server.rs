use crate::convert::{semantic_tokens_legend, to_lsp_diagnostic};
use crate::position::{byte_to_position, encode_semantic_tokens};
use dsql_frontend::{
    AnalysisHost, CompletionKind, DocumentDiagnostics, TextEdit as FrontendTextEdit, TextEditRange,
    TextPosition,
};
use std::error::Error;
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

pub async fn run_stdio() -> std::result::Result<(), Box<dyn Error + Send + Sync>> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
    Ok(())
}

struct Backend {
    client: Client,
    analysis: AnalysisHost,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            analysis: AnalysisHost::new(),
        }
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
}

impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
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
        let result = self
            .analysis
            .open_document(uri.to_string(), version, params.text_document.text)
            .await;
        self.publish_diagnostics(result).await;
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
        if let Some(result) = result {
            self.publish_diagnostics(result).await;
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
