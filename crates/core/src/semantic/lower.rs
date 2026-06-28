use crate::{
    diagnostics::{
        CompilerDiagnostic, CompilerDiagnosticSource, DsqlDiagnostic, extend_compiler_diagnostics,
    },
    language::stages::LowerContext,
    syntax::{
        Argument, DiagnosticCode, DiagnosticSource, Expr, Literal, NameRef, Selection, Severity,
        SourceFile, TextRange, source_span,
    },
};
use facet::Facet;
use lasso::Rodeo;
use miette::LabeledSpan;
use std::collections::HashMap;
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Facet)]
#[repr(transparent)]
pub struct NameId(u32);

#[derive(Debug, Default)]
pub struct Interner {
    strings: Rodeo,
    ids: HashMap<String, NameId>,
    values: Vec<String>,
}

impl Interner {
    pub fn intern(&mut self, value: &str) -> NameId {
        self.strings.get_or_intern(value);
        if let Some(id) = self.ids.get(value).copied() {
            return id;
        }
        let id = NameId(self.values.len() as u32);
        self.values.push(value.to_string());
        self.ids.insert(value.to_string(), id);
        id
    }

    pub fn resolve(&self, id: NameId) -> Option<&str> {
        self.values.get(id.0 as usize).map(String::as_str)
    }
}

#[derive(Clone, Debug, Default, Facet)]
pub struct NameIndex {
    pub queries: Vec<(String, NameId)>,
    pub fragments: Vec<(String, NameId)>,
    pub fields: Vec<(NameId, TextRange)>,
    pub arguments: Vec<(NameId, TextRange)>,
    pub directives: Vec<(NameId, TextRange)>,
}

impl NameIndex {
    /// Inserts a query name and returns a duplicate-name diagnostic when needed.
    pub(crate) fn insert_query(
        &mut self,
        name: &NameRef,
        interner: &mut Interner,
    ) -> Option<LowerDiagnostic> {
        let id = interner.intern(&name.text);
        insert_name(&mut self.queries, &name.text, id).then(|| LowerDiagnostic {
            range: name.range,
            kind: LowerDiagnosticKind::DuplicateQuery {
                name: name.text.clone(),
            },
        })
    }

    /// Inserts a fragment name and returns a duplicate-name diagnostic when needed.
    pub(crate) fn insert_fragment(
        &mut self,
        name: &NameRef,
        interner: &mut Interner,
    ) -> Option<LowerDiagnostic> {
        let id = interner.intern(&name.text);
        insert_name(&mut self.fragments, &name.text, id).then(|| LowerDiagnostic {
            range: name.range,
            kind: LowerDiagnosticKind::DuplicateFragment {
                name: name.text.clone(),
            },
        })
    }
}

#[derive(Clone, Debug, Facet)]
pub struct LoweredFile {
    pub names: NameIndex,
    pub diagnostics: Vec<LowerDiagnostic>,
}

impl CompilerDiagnosticSource for LoweredFile {
    fn extend_compiler_diagnostics(&self, diagnostics: &mut Vec<CompilerDiagnostic>) {
        extend_compiler_diagnostics(diagnostics, self.diagnostics.iter().cloned());
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Facet, thiserror::Error)]
#[repr(C)]
pub enum LowerDiagnosticKind {
    #[error("duplicate query `{name}`")]
    DuplicateQuery { name: String },
    #[error("duplicate fragment `{name}`")]
    DuplicateFragment { name: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Facet)]
pub struct LowerDiagnostic {
    pub range: TextRange,
    pub kind: LowerDiagnosticKind,
}

pub fn lower_file(source_file: &SourceFile, interner: &mut Interner) -> LoweredFile {
    let mut names = NameIndex::default();
    let mut diagnostics = Vec::new();
    let mut context = LowerContext::new(interner, &mut names, &mut diagnostics);
    context.lower_ast_node(source_file.document().into());
    diagnostics.sort_by_key(|diag| (diag.range.start, diag.range.end));
    LoweredFile { names, diagnostics }
}

impl fmt::Display for LowerDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

impl std::error::Error for LowerDiagnostic {}

impl miette::Diagnostic for LowerDiagnostic {
    fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(format!("{:?}", DsqlDiagnostic::code(self))))
    }

    fn severity(&self) -> Option<miette::Severity> {
        Some(miette::Severity::Error)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        Some(Box::new(std::iter::once(LabeledSpan::underline(
            source_span(self.range),
        ))))
    }
}

impl DsqlDiagnostic for LowerDiagnostic {
    fn range(&self) -> TextRange {
        self.range
    }

    fn severity(&self) -> Severity {
        Severity::Error
    }

    fn code(&self) -> DiagnosticCode {
        DiagnosticCode::DuplicateDefinition
    }

    fn source(&self) -> DiagnosticSource {
        DiagnosticSource::Lower
    }
}

fn insert_name(names: &mut Vec<(String, NameId)>, text: &str, id: NameId) -> bool {
    if names.iter().any(|(name, _)| name == text) {
        true
    } else {
        names.push((text.to_string(), id));
        false
    }
}

/// Lowers a legacy selection list while selection ownership is migrating to atoms.
pub(crate) fn lower_selection_list(selections: &[Selection], context: &mut LowerContext<'_>) {
    for selection in selections {
        match selection {
            Selection::Field(field) => context.lower_ast_node(field.into()),
            Selection::FragmentSpread(spread) => context.lower_ast_node(spread.into()),
        }
    }
}

pub(crate) fn lower_argument(argument: &Argument, context: &mut LowerContext<'_>) {
    context.names.arguments.push((
        context.interner.intern(&argument.name.text),
        argument.name.range,
    ));
    lower_expr(&argument.value, context.interner);
}

pub(crate) fn lower_selection_clauses(clauses: &[crate::Clause], context: &mut LowerContext<'_>) {
    for clause in clauses {
        if let crate::Clause::OrderBy(order_by) = clause {
            for item in &order_by.items {
                if let crate::SortDirectionExpr::Variable(variable) = &item.direction
                    && let Some(name) = &variable.name
                {
                    context.interner.intern(&name.text);
                }
            }
        }
    }
}

fn lower_expr(expr: &Expr, interner: &mut Interner) {
    match expr {
        Expr::Name(name) => {
            interner.intern(&name.text);
        }
        Expr::Path(path) => {
            for segment in &path.segments {
                interner.intern(&segment.name.text);
                if let Some(selector) = &segment.selector {
                    interner.intern(&selector.text);
                }
            }
        }
        Expr::Binary {
            left, op, right, ..
        } => {
            lower_expr(left, interner);
            if let crate::BinaryOperator::Variable(variable) = op
                && let Some(name) = &variable.name
            {
                interner.intern(&name.text);
            }
            lower_expr(right, interner);
        }
        Expr::Variable(variable) => {
            if let Some(name) = &variable.name {
                interner.intern(&name.text);
            }
        }
        Expr::Literal(
            Literal::String { .. }
            | Literal::Number { .. }
            | Literal::Bool { .. }
            | Literal::Null { .. },
        ) => {}
    }
}
