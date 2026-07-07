//! The dsql grammar: logos lexer, lelwel-generated parser, and the parse
//! entry point producing a lossless CST.
//!
//! The generated [`parser::Rule`] enum is the single source of truth for
//! language constructs; nothing in this crate mirrors it. The hand-written
//! [`lexer`] duplicates the token strings declared in `dsql.llw` until the
//! vendored lelwel learns to emit it (docs/plan.md, phase 8).

pub mod lexer;
pub mod parser;

use parser::{Cst, Diagnostic, Parser};

/// Parses dsql source text into a lossless CST plus parse diagnostics.
///
/// The CST retains all trivia (whitespace, comments) and error nodes, so it
/// is suitable for formatting and editor services as well as lowering.
pub fn parse(source: &str) -> (Cst<'_>, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    let cst = Parser::new(source, &mut diagnostics).parse(&mut diagnostics);
    (cst, diagnostics)
}
