//! Editor services: request/response facts external callers drive.

pub mod completion;
pub mod definition;
pub mod hover;
pub mod semantic_tokens;

use bowl::Bowl;

pub use completion::{
    CompletionCandidate, CompletionContext, CompletionItem, CompletionKind, CompletionList,
    CompletionRequest, CompletionSite,
};
pub use definition::{DefinitionRequest, DefinitionTarget};
pub use hover::{HoverCandidate, HoverEnriched, HoverInfo, HoverRequest, Position, priority};
pub use semantic_tokens::{SemanticToken, SemanticTokenKind, SemanticTokens, SemanticTokensRequest};

/// Registers the service pipelines shared by all entities.
pub async fn register_services(bowl: &Bowl) {
    completion::register_completion_pipeline(bowl).await;
    hover::register_hover_pipeline(bowl).await;
    definition::register_definition_pipeline(bowl).await;
    semantic_tokens::register_semantic_tokens_pipeline(bowl).await;
}
