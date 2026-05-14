use dsql_core::{
    Catalog, Diagnostic, Interner, SourceSnapshot, check_file_with_catalog, lint_file_with_catalog,
    lower_file, parse_source, plan_file_with_catalog,
};

use crate::db::AnalysisResult;

pub fn analyze_source(source: SourceSnapshot) -> AnalysisResult {
    let parse = parse_source(source);
    let mut interner = Interner::default();
    let lower = lower_file(&parse.source_file, &mut interner);
    let check = check_file_with_catalog(&parse.source_file, &Catalog::hardcoded());
    let lint = lint_file_with_catalog(&parse.source_file, &Catalog::hardcoded());
    let plan = plan_file_with_catalog(&parse.source_file, &Catalog::hardcoded());
    AnalysisResult {
        parse,
        lower,
        check,
        lint,
        plan,
    }
}

pub fn collect_diagnostics(analysis: &AnalysisResult) -> Vec<Diagnostic> {
    collect_diagnostics_parts(
        &analysis.parse.diagnostics,
        &analysis.lower.diagnostics,
        &analysis.check.diagnostics,
        &analysis.lint.diagnostics,
    )
}

pub(crate) fn collect_diagnostics_parts(
    parse: &[Diagnostic],
    lower: &[Diagnostic],
    check: &[Diagnostic],
    lint: &[Diagnostic],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    diagnostics.extend(parse.iter().cloned());
    diagnostics.extend(lower.iter().cloned());
    diagnostics.extend(check.iter().cloned());
    diagnostics.extend(lint.iter().cloned());
    diagnostics.sort_by_key(|diag| (diag.range.start, diag.range.end));
    diagnostics
}
