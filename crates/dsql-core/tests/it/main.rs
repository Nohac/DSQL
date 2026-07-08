//! Integration test harness for `dsql-core`. One binary, one module per
//! area, mirroring the layout described in CONTRIBUTING.md.

mod checks;
mod completions;
mod definitions;
mod format;
mod lowering;
mod parse;
mod scopes;
mod selections;
mod services;
mod settle;
mod sql;
mod variables;
mod support;

pub use support::{fixture, imdb_catalog, queries_dir, render_diagnostic_facts, render_diagnostics};
