//! Editor services: request/response facts external callers drive.

pub mod completion;
pub mod definition;
pub mod hover;
pub mod semantic_tokens;

use bowl::Registrar;

pub use completion::{
    CompletionCandidate, CompletionContext, CompletionItem, CompletionKind, CompletionList,
    CompletionRequest, CompletionSite, DirectiveCompletionContext, DirectiveRole,
    PolicyCompletionContext, PolicyCompletionRole, PolicyCompletionTarget,
};
pub use definition::{CatalogDefinition, DefinitionRequest, DefinitionTarget};
pub use hover::{
    Cursor, HoverCandidate, HoverEnriched, HoverInfo, HoverRequest, Position, priority,
};
pub use semantic_tokens::{
    SemanticToken, SemanticTokenKind, TokenChunk, TokensDemand, semantic_tokens,
};

/// Registers the service pipelines shared by all entities.
pub fn register_services(reg: &mut Registrar<'_>) {
    completion::register_completion_pipeline(reg);
    hover::register_hover_pipeline(reg);
    definition::register_definition_pipeline(reg);
    semantic_tokens::register_semantic_tokens_pipeline(reg);
}
