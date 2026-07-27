//! Integration test harness for `dsql-core`. One binary, one module per
//! area, mirroring the layout described in CONTRIBUTING.md.

mod aggregates;
mod catalog;
mod checks;
mod completions;
mod definitions;
mod embedding;
mod format;
mod input;
mod lints;
mod lowering;
mod parse;
mod policies;
mod residency;
mod scale;
mod scenarios;
mod scopes;
mod selections;
mod services;
mod settle;
mod sql;
mod support;
mod variables;

pub use support::{
    fixture, imdb_catalog, numeric_catalog, policy_completion_catalog, provider_scalar_catalog,
    render_diagnostic_facts, render_diagnostics, replace_source_text, set_source_text,
};
