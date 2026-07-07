//! Variable entity: every `$name` / `$$name` occurrence as a fact.
//!
//! Variables live inside expression trees structurally (see `expression`),
//! but inference is set-oriented — "which parameters does this query take,
//! at which binding time, with which types" — so each occurrence also
//! becomes its own fact, anchored into the tree by [`ParentKey`].

use bowl::{Bowl, Commands, Component, DerivedFrom};

use crate::entities::expression::{Sigil, VariableRef, build_variable_ref};
use crate::entity::{LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{BelongsToFile, NodeKey, ParentKey};
use crate::grammar::parser::NodeRef;

/// One variable occurrence, lowered from `value_variable` or
/// `operator_variable`. The inference stage (phase 7) groups these by name
/// and derives the query's parameter set.
#[derive(Component, Debug, Hash)]
#[component(hash)]
pub struct VariableUse(pub VariableRef);

impl VariableUse {
    pub fn sigil(&self) -> Sigil {
        self.0.sigil
    }
}

/// Owns `value_variable` and `operator_variable`.
pub struct Variable;

impl LanguageEntity for Variable {
    const NAME: &'static str = "variable";

    async fn register(_db: &Bowl) {
        // Variable inference systems land in phase 7.
    }
}

impl LowerStage for Variable {
    fn lower(ctx: &LowerCtx<'_>, node: NodeRef, commands: &mut Commands) {
        let variable = build_variable_ref(ctx.cst, ctx.source, node);

        let key = NodeKey {
            file: ctx.file,
            node: node.0,
        };

        match ctx.parent {
            Some(parent) => commands.insert((
                DerivedFrom::new(ctx.file),
                BelongsToFile(ctx.file),
                key,
                ParentKey(parent),
                VariableUse(variable),
            )),
            None => commands.insert((
                DerivedFrom::new(ctx.file),
                BelongsToFile(ctx.file),
                key,
                VariableUse(variable),
            )),
        };
    }
}
