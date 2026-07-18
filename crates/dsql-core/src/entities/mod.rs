//! Language entities: one vertical slice per language concept, plus the
//! shared lowering walk and rule-ownership dispatch (see [`lowering`]).

pub mod aggregate;
pub mod clause;
pub mod definition;
pub mod directive;
pub mod document;
pub(crate) mod expansion;
pub mod expression;
pub mod field_selection;
pub mod fragment_spread;
pub mod lowering;
pub mod variable;
pub mod variable_path;

pub(crate) use lowering::{direct_name, direct_names, direct_rule, direct_token, node_span, text};
pub use lowering::{format_rule, lower_syntax_facts};
