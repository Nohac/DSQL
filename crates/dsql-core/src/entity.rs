//! The language-entity contract.
//!
//! A language entity is a vertical slice of one language concept: a single
//! file co-locates the entity's fact components, how they lower from the
//! CST, the checks that validate them, and how they present to services.
//! This carries the language-atom guarantees in the porridge
//! playground shape: plain traits and one exhaustive `match`, no registry.
//!
//! The stage traits below form the coverage contract. [`register_entity`]
//! bounds on every stage, so a new entity does not compile until it has
//! declared each one — a stage that does not apply is an explicit no-effect
//! impl whose doc comment states why. Stage traits are added in the phase
//! that introduces the stage (see docs/plan.md), which retroactively forces
//! every existing entity to declare it.
//!
//! Grammar-rule ownership lives in `entities::lower_rule`: an exhaustive
//! `match` on the generated [`Rule`] enum, so adding a rule to `dsql.llw`
//! fails to compile until an entity claims it or it is explicitly listed as
//! structural.
//!
//! [`Rule`]: crate::grammar::parser::Rule

use bowl::{Commands, Entity, Registrar};

use crate::schema::AstFacts;

use crate::format::CstFormatter;
use crate::grammar::parser::{CstData, NodeRef};

/// Identity and system registration for one language concept.
pub trait LanguageEntity {
    const NAME: &'static str;

    /// Registers the entity's derivation and check systems.
    fn register(reg: &mut Registrar<'_>);
}

/// Context handed to [`LowerStage::lower`] for one owned CST rule node.
pub struct LowerCtx<'a> {
    /// The parsed tree the node belongs to.
    pub cst: &'a CstData,
    /// The exact source text the tree was parsed from (same revision), so
    /// span slicing is always in bounds.
    pub source: &'a str,
    /// The file entity the tree was parsed from; fact components emitted by
    /// lowering anchor to it.
    pub file: Entity,
    /// The file's resolution scope; definition and spread facts carry it
    /// as their resolution join key.
    pub scope: &'a str,
    /// Entity of the nearest enclosing selection or definition fact, if
    /// any. The walk in `entities` scopes it when descending into one, so
    /// nested facts carry their tree position as a [`ChildOf`] edge and
    /// the engine maintains the parent's [`Children`] inverse.
    ///
    /// [`ChildOf`]: crate::facts::ChildOf
    /// [`Children`]: crate::facts::Children
    pub parent: Option<Entity>,
}

/// Syntax stage: lower an owned CST rule node into fact components,
/// returning the created fact entity when the node forms a tree position
/// descendants attach to. Rule ownership is assigned in
/// `entities::lower_rule`.
pub trait LowerStage: LanguageEntity {
    fn lower(
        ctx: &LowerCtx<'_>,
        node: NodeRef,
        commands: &mut Commands<AstFacts>,
    ) -> Option<Entity>;
}

/// Format stage: write the canonical text of an owned CST rule node.
/// Ownership is assigned in `entities::format_rule`; the engine
/// ([`CstFormatter`]) owns layout decisions the entities share.
pub trait FormatStage: LanguageEntity {
    fn format(formatter: &mut CstFormatter<'_>, node: NodeRef);
}

/// Service stage: register systems that answer hover requests by inserting
/// `HoverCandidate` facts (see `service::hover` for the pipeline). Each
/// entity's systems read only the enriched request plus the entity's own
/// facts, so param lists stay small no matter how many entities exist.
/// Entities without hover content register nothing (explicit empty impl).
pub trait HoverStage: LanguageEntity {
    fn register_hover(reg: &mut Registrar<'_>);
}

/// Service stage: register systems that contribute `CompletionCandidate`
/// facts for enriched completion requests (see `service::completion`).
/// Entities without completions register nothing (explicit empty impl).
pub trait CompletionStage: LanguageEntity {
    fn register_completions(reg: &mut Registrar<'_>);
}

/// The compile-time coverage contract: an entity only registers once it has
/// declared every stage.
pub fn register_entity<E>(reg: &mut Registrar<'_>)
where
    E: LanguageEntity + LowerStage + FormatStage + HoverStage + CompletionStage,
{
    E::register(reg);
    E::register_hover(reg);
    E::register_completions(reg);
}
