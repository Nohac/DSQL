//! The language-entity contract.
//!
//! A language entity is a vertical slice of one language concept: a single
//! file co-locates the entity's fact components, how they lower from the
//! CST, the checks that validate them, and how they present to services.
//! This carries the dsql-poc "language atom" guarantees in the porridge
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

use bowl::{Bowl, Commands, Entity};

use crate::facts::NodeKey;
use crate::grammar::parser::{CstData, NodeRef};

/// Identity and system registration for one language concept.
pub trait LanguageEntity {
    const NAME: &'static str;

    /// Registers the entity's derivation and check systems on the bowl.
    fn register(bowl: &Bowl) -> impl Future<Output = ()> + Send;
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
    /// Key of the nearest enclosing selection or definition node, if any.
    /// The walk in `entities` scopes it when descending into one, so nested
    /// facts carry their tree position as a [`ParentKey`].
    ///
    /// [`ParentKey`]: crate::facts::ParentKey
    pub parent: Option<NodeKey>,
}

/// Syntax stage: lower an owned CST rule node into fact components.
/// Rule ownership is assigned in `entities::lower_rule`.
pub trait LowerStage: LanguageEntity {
    fn lower(ctx: &LowerCtx<'_>, node: NodeRef, commands: &mut Commands);
}

/// The compile-time coverage contract: an entity only registers once it has
/// declared every stage.
pub async fn register_entity<E>(bowl: &Bowl)
where
    E: LanguageEntity + LowerStage,
{
    E::register(bowl).await;
}
