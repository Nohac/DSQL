//! The dsql language: grammar, language entities, semantic stages, and
//! services, built on the porridge (`bowl`) incremental engine.
//!
//! See `docs/plan.md` at the repository root for the architecture.

pub mod entities;
pub mod entity;
pub mod facts;
pub mod grammar;
pub mod source;

use bowl::{Bowl, Phase, SystemExt, cleanup_stale_derived};

use crate::entities::definition::Definition;
use crate::entities::document::Document;
use crate::entity::register_entity;

/// Assembles the language on a bowl: every entity, the shared lowering
/// walk, and the cleanup systems the derived-fact conventions rely on.
pub async fn register_language(db: &Bowl) {
    register_entity::<Document>(db).await;
    register_entity::<Definition>(db).await;

    db.add_system(entities::generate_ast).await;

    db.add_system(cleanup_stale_derived.run_during(Phase::Cleanup))
        .await;
}
