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
pub mod resolution;
pub mod schema;
pub mod service;
pub mod source;
pub mod sql;

use bowl::{
    Bowl, Phase, Plugin, Registrar, Schema, ShapeDesc, Singleton, SystemExt, cleanup_stale_derived,
};

use crate::entities::clause::Clause;
use crate::entities::definition::Definition;
use crate::entities::directive::Directive;
use crate::entities::document::Document;
use crate::entities::expression::Expression;
use crate::entities::field_selection::FieldSelection;
use crate::entities::fragment_spread::FragmentSpread;
use crate::entities::variable::Variable;
use crate::entity::register_entity;

/// The dsql language as a bowl plugin: its schema and every entity, the
/// shared lowering walk, the semantic stages, the services, and the
/// cleanup system the derived-fact conventions rely on — shapes and
/// systems travel together, so installing the plugin cannot desync them.
pub struct DsqlPlugin;

impl Plugin for DsqlPlugin {
    fn shapes(&self) -> Vec<ShapeDesc> {
        schema::DsqlSchema::shapes()
    }

    fn build(&self, reg: &mut Registrar<'_>) {
        register_entity::<Document>(reg);
        register_entity::<Definition>(reg);
        register_entity::<FieldSelection>(reg);
        register_entity::<FragmentSpread>(reg);
        register_entity::<Clause>(reg);
        register_entity::<Directive>(reg);
        register_entity::<Expression>(reg);
        register_entity::<Variable>(reg);

        reg.system(entities::generate_ast);

        embedding::register_embedding(reg);

        lint::register_lints(reg);
        plan::register_planning(reg);
        resolution::register_resolution(reg);
        sql::register_sql(reg);
        service::register_services(reg);

        reg.system(cleanup_stale_derived.run_during(Phase::Settle));
    }
}

/// Builds a language bowl and installs the default singletons project
/// loading otherwise replaces (scope configuration, embedding pattern).
pub async fn language_bowl() -> Bowl {
    let bowl = Bowl::builder().plugin(DsqlPlugin).build();
    install_default_singletons(&bowl).await;
    bowl
}

/// Installs the default configuration singletons on an already-built
/// bowl — for callers that must construct synchronously (the LSP backend)
/// and arm defaults later. Project loading replaces them.
pub async fn install_default_singletons(bowl: &Bowl) {
    bowl.insert((
        Singleton::<source::ScopeImports>::new(),
        source::ScopeImports::default(),
    ))
    .await;
    bowl.insert((
        Singleton::<source::ScopeDocuments>::new(),
        source::ScopeDocuments::default(),
    ))
    .await;
    bowl.insert((
        Singleton::<embedding::EmbeddedPattern>::new(),
        embedding::EmbeddedPattern::default(),
    ))
    .await;
    bowl.insert((
        Singleton::<sql::SqlOptions>::new(),
        sql::SqlOptions::default(),
    ))
    .await;
}
