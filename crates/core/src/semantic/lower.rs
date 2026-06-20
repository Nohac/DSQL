use crate::{
    diagnostics::{
        CompilerDiagnostic, CompilerDiagnosticSource, DsqlDiagnostic, extend_compiler_diagnostics,
    },
    syntax::{
        Argument, Definition, DiagnosticCode, DiagnosticSource, Document, Expr, FragmentDef,
        Literal, QueryDef, Selection, Severity, SourceFile, TextRange, source_span,
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
    lower_document(
        source_file.document(),
        interner,
        &mut names,
        &mut diagnostics,
    );
    diagnostics.sort_by_key(|diag| (diag.range.start, diag.range.end));
    LoweredFile { names, diagnostics }
}

fn lower_document(
    document: &Document,
    interner: &mut Interner,
    names: &mut NameIndex,
    diagnostics: &mut Vec<LowerDiagnostic>,
) {
    for definition in &document.definitions {
        match definition {
            Definition::Query(query) => lower_query(query, interner, names, diagnostics),
            Definition::Fragment(fragment) => {
                lower_fragment(fragment, interner, names, diagnostics)
            }
        }
    }
}

fn lower_query(
    query: &QueryDef,
    interner: &mut Interner,
    names: &mut NameIndex,
    diagnostics: &mut Vec<LowerDiagnostic>,
) {
    if let Some(name) = &query.name {
        let id = interner.intern(&name.text);
        if insert_name(&mut names.queries, &name.text, id) {
            diagnostics.push(LowerDiagnostic {
                range: name.range,
                kind: LowerDiagnosticKind::DuplicateQuery {
                    name: name.text.clone(),
                },
            });
        }
    }
    lower_selections(&query.selections, interner, names);
}

fn lower_fragment(
    fragment: &FragmentDef,
    interner: &mut Interner,
    names: &mut NameIndex,
    diagnostics: &mut Vec<LowerDiagnostic>,
) {
    if let Some(name) = &fragment.name {
        let id = interner.intern(&name.text);
        if insert_name(&mut names.fragments, &name.text, id) {
            diagnostics.push(LowerDiagnostic {
                range: name.range,
                kind: LowerDiagnosticKind::DuplicateFragment {
                    name: name.text.clone(),
                },
            });
        }
    }
    if let Some(on) = &fragment.on {
        interner.intern(&on.display_text());
    }
    lower_selections(&fragment.selections, interner, names);
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

fn lower_selections(selections: &[Selection], interner: &mut Interner, names: &mut NameIndex) {
    for selection in selections {
        if let Some(alias) = &selection.alias {
            interner.intern(&alias.text);
        }
        names.fields.push((
            interner.intern(&selection.name.display_text()),
            selection.name.range,
        ));
        for argument in &selection.arguments {
            lower_argument(argument, interner, names);
        }
        for directive in &selection.directives {
            names
                .directives
                .push((interner.intern(&directive.text), directive.range));
        }
        for clause in &selection.clauses {
            if let crate::Clause::OrderBy(order_by) = clause {
                for item in &order_by.items {
                    if let crate::SortDirectionExpr::Variable(variable) = &item.direction
                        && let Some(name) = &variable.name
                    {
                        interner.intern(&name.text);
                    }
                }
            }
        }
        lower_selections(&selection.selections, interner, names);
    }
}

fn lower_argument(argument: &Argument, interner: &mut Interner, names: &mut NameIndex) {
    names
        .arguments
        .push((interner.intern(&argument.name.text), argument.name.range));
    lower_expr(&argument.value, interner);
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
