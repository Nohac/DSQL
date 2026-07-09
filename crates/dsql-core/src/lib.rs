//! The dsql language: grammar, language entities, semantic stages, and
//! services, built on the porridge (`bowl`) incremental engine.
//!
//! See `docs/plan.md` at the repository root for the architecture.

pub mod catalog;
pub mod embedding;
pub mod entities;
pub mod entity;
pub mod facts;
pub mod format;
pub mod grammar;
pub mod lint;
pub mod plan;
pub mod service;
pub mod source;
pub mod sql;

use bowl::{Bowl, Phase, Singleton, SystemExt, cleanup_stale_derived};

use crate::entities::clause::Clause;
use crate::entities::definition::Definition;
use crate::entities::directive::Directive;
use crate::entities::document::Document;
use crate::entities::expression::Expression;
use crate::entities::field_selection::FieldSelection;
use crate::entities::fragment_spread::FragmentSpread;
use crate::entities::variable::Variable;
use crate::entity::register_entity;

/// Assembles the language on a bowl: every entity, the shared lowering
/// walk, and the cleanup systems the derived-fact conventions rely on.
pub async fn register_language(bowl: &Bowl) {
    // Default scope configuration; project loading replaces it.
    bowl.insert((
        Singleton::<source::ScopeImports>::new(),
        source::ScopeImports::default(),
    ))
    .await;

    register_entity::<Document>(bowl).await;
    register_entity::<Definition>(bowl).await;
    register_entity::<FieldSelection>(bowl).await;
    register_entity::<FragmentSpread>(bowl).await;
    register_entity::<Clause>(bowl).await;
    register_entity::<Directive>(bowl).await;
    register_entity::<Expression>(bowl).await;
    register_entity::<Variable>(bowl).await;

    bowl.add_system(entities::generate_ast).await;

    embedding::register_embedding(bowl).await;

    lint::register_lints(bowl).await;
    plan::register_planning(bowl).await;
    sql::register_sql(bowl).await;
    service::register_services(bowl).await;

    bowl.add_system(cleanup_stale_derived.run_during(Phase::Settle))
        .await;
}
