use crate::syntax::{
    Argument, Definition, Diagnostic, DiagnosticCode, DiagnosticSource, Document, Expr,
    FragmentDef, Literal, QueryDef, Selection, Severity, SourceFile, TextRange,
};
use facet::Facet;
use lasso::Rodeo;
use std::collections::HashMap;

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
    pub diagnostics: Vec<Diagnostic>,
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
    diagnostics: &mut Vec<Diagnostic>,
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
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(name) = &query.name {
        let id = interner.intern(&name.text);
        if insert_name(&mut names.queries, &name.text, id) {
            diagnostics.push(Diagnostic {
                range: name.range,
                severity: Severity::Error,
                code: DiagnosticCode::DuplicateDefinition,
                message: format!("duplicate query `{}`", name.text),
                source: DiagnosticSource::Lower,
            });
        }
    }
    lower_selections(&query.selections, interner, names);
}

fn lower_fragment(
    fragment: &FragmentDef,
    interner: &mut Interner,
    names: &mut NameIndex,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(name) = &fragment.name {
        let id = interner.intern(&name.text);
        if insert_name(&mut names.fragments, &name.text, id) {
            diagnostics.push(Diagnostic {
                range: name.range,
                severity: Severity::Error,
                code: DiagnosticCode::DuplicateDefinition,
                message: format!("duplicate fragment `{}`", name.text),
                source: DiagnosticSource::Lower,
            });
        }
    }
    if let Some(on) = &fragment.on {
        interner.intern(&on.text);
    }
    lower_selections(&fragment.selections, interner, names);
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
        names
            .fields
            .push((interner.intern(&selection.name.text), selection.name.range));
        for argument in &selection.arguments {
            lower_argument(argument, interner, names);
        }
        for directive in &selection.directives {
            names
                .directives
                .push((interner.intern(&directive.text), directive.range));
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
        Expr::Binary { left, right, .. } => {
            lower_expr(left, interner);
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
