use tower_lsp_server::ls_types::{SemanticTokenType, SemanticTokensLegend};

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
