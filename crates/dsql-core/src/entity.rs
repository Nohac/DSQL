//! The language-entity contract.
//!
//! A language entity is a vertical slice of one language concept: a single
//! file co-locates the entity's fact components, how they lower from the
//! CST, the checks that validate them, and how they present to services.
//! This carries the language-atom guarantees in the porridge
//! playground shape: plain traits and one exhaustive `match`, no registry.
//!
//! The stage traits below cover *syntax*: lowering and formatting join
//! the exhaustive generated-rule dispatch, so a new construct cannot
//! compile without them. Semantic and service behavior registers as
//! ordinary systems inside [`LanguageEntity::register`] — empty per-stage
//! trait impls proved nothing and are gone; the bowl schema and declared
//! system outputs are the coverage contract for behavior.
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

/// The compile-time coverage contract: an entity registers once it has
/// declared the *syntax* stages — lowering and formatting participate in
/// exhaustive generated-rule ownership, so a new construct cannot compile
/// without them. Semantic and service systems are ordinary plugin
/// registrations inside [`LanguageEntity::register`]; empty trait impls
/// proved nothing about their coverage and are gone.
pub fn register_entity<E>(reg: &mut Registrar<'_>)
where
    E: LanguageEntity + LowerStage + FormatStage,
{
    E::register(reg);
}
