//! Editor services: request/response facts external callers drive.

pub mod definition;
pub mod hover;

use bowl::Bowl;

pub use definition::{DefinitionRequest, DefinitionTarget};
pub use hover::{HoverCandidate, HoverEnriched, HoverInfo, HoverRequest, Position, priority};

/// Registers the service pipelines shared by all entities.
pub async fn register_services(bowl: &Bowl) {
    hover::register_hover_pipeline(bowl).await;
    definition::register_definition_pipeline(bowl).await;
}
