use crate::position::byte_to_position;
use dsql_core::Diagnostic;
use ropey::Rope;
use tower_lsp_server::lsp_types::{
    Diagnostic as LspDiagnostic, DiagnosticSeverity, Range, SemanticTokenType, SemanticTokensLegend,
};

pub(crate) fn semantic_tokens_legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::NAMESPACE,
            SemanticTokenType::CLASS,
            SemanticTokenType::new("relation"),
            SemanticTokenType::PROPERTY,
            SemanticTokenType::new("fragment"),
            SemanticTokenType::new("alias"),
        ],
        token_modifiers: Vec::new(),
    }
}

pub(crate) fn to_lsp_diagnostic(diagnostic: &Diagnostic, rope: &Rope) -> LspDiagnostic {
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
