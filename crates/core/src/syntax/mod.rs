mod ast;
mod diagnostics;
pub(crate) mod grammar;
pub(crate) mod parse;
mod text;

pub use crate::language::atoms::directive::{
    Directive, DirectiveArgumentDefinition, DirectiveArgumentValueKind, DirectiveLocation,
    SystemDirectiveDefinition, SystemDirectiveKind,
};
pub use crate::language::atoms::document::{Definition, Document, SourceFile};
pub use crate::language::atoms::field_selection::FieldSelection;
pub use crate::language::atoms::fragment_def::FragmentDef;
pub use crate::language::atoms::fragment_spread::FragmentSpread;
pub use crate::language::atoms::query_def::QueryDef;
pub use ast::{
    Argument, BinaryOp, BinaryOperator, Clause, Expr, LimitClause, Literal, NameRef, OffsetClause,
    OperatorVariable, OrderByClause, OrderByItem, PathScope, QualifiedNameRef, RelationRef,
    ScopedPath, ScopedPathSegment, Selection, SortDirection, SortDirectionExpr, ValueVariable,
    VariableScope, WhereClause,
};
pub(crate) use diagnostics::source_span;
pub use diagnostics::{Diagnostic, DiagnosticCode, DiagnosticSource, Severity};
pub use grammar::lexer::Token;
pub use parse::{
    CstKind, ParseResult, SyntaxNode, SyntaxRule, SyntaxToken, SyntaxTree, expected_tokens_at,
    parse_source,
};
pub use text::{SourceDocument, SourceRegion, SourceSnapshot, TextRange};
