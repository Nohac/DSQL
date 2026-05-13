use dsql_core::Diagnostic;
use dsql_frontend::{
    AnalysisHost, CompletionKind, DocumentDiagnostics, TextEdit as FrontendTextEdit, TextEditRange,
    TextPosition,
};
use ropey::{LineType, Rope};
use std::error::Error;
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams, CompletionResponse,
    Diagnostic as LspDiagnostic, DiagnosticSeverity, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DocumentFormattingParams, Hover,
    HoverContents, HoverParams, InitializeParams, InitializeResult, InitializedParams,
    MarkedString, MessageType, OneOf, Position, Range, ServerCapabilities,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, Uri,
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
        let uri = parse_uri(&result.snapshot.uri);
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
            contents: HoverContents::Scalar(MarkedString::String(format!(
                "{}\n{}",
                info.label, info.detail
            ))),
            range: None,
        }))
    }
}

fn parse_uri(value: &str) -> Option<Uri> {
    value.parse().ok()
}

fn to_lsp_diagnostic(diagnostic: &Diagnostic, rope: &Rope) -> LspDiagnostic {
    LspDiagnostic {
        range: Range {
            start: byte_to_position(rope, diagnostic.range.start as usize),
            end: byte_to_position(rope, diagnostic.range.end as usize),
        },
        severity: Some(match diagnostic.severity {
            dsql_core::Severity::Error => DiagnosticSeverity::ERROR,
            dsql_core::Severity::Warning => DiagnosticSeverity::WARNING,
            dsql_core::Severity::Info => DiagnosticSeverity::INFORMATION,
        }),
        code: Some(tower_lsp_server::lsp_types::NumberOrString::String(
            format!("{:?}", diagnostic.code),
        )),
        source: Some("dsql".to_string()),
        message: diagnostic.message.clone(),
        ..LspDiagnostic::default()
    }
}

fn byte_to_position(rope: &Rope, byte: usize) -> Position {
    let byte = byte.min(rope.len());
    let line = rope.byte_to_line_idx(byte, LineType::LF_CR);
    let line_start = rope.line_to_byte_idx(line, LineType::LF_CR);
    let character = rope.slice(line_start..byte).len_utf16();
    Position::new(line as u32, character as u32)
}
