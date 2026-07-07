//! Integration test harness for `dsql-core`. One binary, one module per
//! area, mirroring the layout described in CONTRIBUTING.md.

mod definitions;
mod lowering;
mod parse;
mod selections;
mod settle;
mod support;

pub use support::{fixture, queries_dir, render_diagnostic_facts, render_diagnostics};
