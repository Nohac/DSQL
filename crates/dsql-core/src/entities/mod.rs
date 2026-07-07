//! Language entities: one vertical slice per language concept, plus the
//! shared lowering walk and rule-ownership dispatch (see [`lowering`]).

pub mod clause;
pub mod definition;
pub mod directive;
pub mod document;
pub mod expression;
pub mod field_selection;
pub mod fragment_spread;
pub mod lowering;
pub mod variable;

pub use lowering::generate_ast;
pub(crate) use lowering::{direct_rule, direct_token, node_span, text};
